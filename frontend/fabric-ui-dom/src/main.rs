#![forbid(unsafe_code)]
use fabric_app_core::{Effect, Event, MAX_RESPONSE_BYTES, Phase, State};
use fabric_query_contract::{
    API_VERSION, Budgets, Filter, Operation, QueryRequest, RunKey, RunSummary,
};
use leptos::{prelude::*, reactive::owner::LocalStorage};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    AbortController, ReadableStreamDefaultReader, Request, RequestCredentials, RequestInit,
    RequestMode, RequestRedirect, Response,
};

// The timer is cleared on every return path, including body errors.
struct Deadline {
    window: web_sys::Window,
    handle: i32,
    _callback: Closure<dyn FnMut()>,
}
impl Drop for Deadline {
    fn drop(&mut self) {
        self.window.clear_timeout_with_handle(self.handle);
    }
}
fn focus_results() {
    if let Some(window) = web_sys::window() {
        let callback = Closure::once_into_js(move || {
            if let Some(element) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("results-heading"))
                && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = element.focus();
            }
        });
        let _ = window.request_animation_frame(callback.unchecked_ref());
    }
}
// A gateway owns authentication and authorization. The browser sends no canonical credential.
async fn fetch_query(
    mut query: QueryRequest,
    controller: AbortController,
) -> Result<Vec<u8>, &'static str> {
    let window = web_sys::window().ok_or("Browser unavailable")?;
    // Request labels separate simultaneous reads across browser instances.
    // Scope and snapshot remain the reducer's exact typed selection.
    let mut entropy = [0; 16];
    window
        .crypto()
        .and_then(|crypto| crypto.get_random_values_with_u8_array(&mut entropy))
        .map_err(|_| "Could not label request")?;
    query.query_id = format!("ui-{:032x}", u128::from_be_bytes(entropy));
    let body = serde_json::to_string(&query).map_err(|_| "Invalid request")?;
    let deadline_controller = controller.clone();
    let callback = Closure::wrap(Box::new(move || deadline_controller.abort()) as Box<dyn FnMut()>);
    // One absolute deadline covers both headers and every body chunk.
    let handle = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            15_000,
        )
        .map_err(|_| "Could not start request deadline")?;
    let _deadline = Deadline {
        window: window.clone(),
        handle,
        _callback: callback,
    };
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_mode(RequestMode::SameOrigin);
    init.set_redirect(RequestRedirect::Error);
    init.set_signal(Some(&controller.signal()));
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_body(&JsValue::from_str(&body));
    let request = Request::new_with_str_and_init("/api/v1/query", &init)
        .map_err(|_| "Could not create request")?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|_| "Could not create request")?;
    let response: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| {
            if controller.signal().aborted() {
                "The query timed out. Its outcome is unknown."
            } else {
                "The network is unavailable. The query outcome is unknown."
            }
        })?
        .dyn_into()
        .map_err(|_| "Invalid response")?;
    if response.status() == 401 || response.status() == 403 {
        return Err("Access denied or session expired");
    }
    if !response.ok() {
        return Err("Query failed. Check the gateway and try again.");
    }
    let stream = response.body().ok_or("Response body unavailable")?;
    let reader: ReadableStreamDefaultReader = stream
        .get_reader()
        .dyn_into()
        .map_err(|_| "Response body unavailable")?;
    let mut bytes = Vec::new();
    loop {
        let item = JsFuture::from(reader.read()).await.map_err(|_| {
            if controller.signal().aborted() {
                "The query timed out. Its outcome is unknown."
            } else {
                "The connection was interrupted. The query outcome is unknown."
            }
        })?;
        let done = js_sys::Reflect::get(&item, &JsValue::from_str("done"))
            .map_err(|_| "Invalid response")?
            .as_bool()
            .ok_or("Invalid response")?;
        if done {
            break;
        }
        let value = js_sys::Reflect::get(&item, &JsValue::from_str("value"))
            .map_err(|_| "Invalid response")?;
        let chunk = js_sys::Uint8Array::new(&value);
        let length = usize::try_from(chunk.length()).map_err(|_| "Response too large")?;
        if bytes
            .len()
            .checked_add(length)
            .is_none_or(|n| n > MAX_RESPONSE_BYTES || n > query.budgets.output_bytes as usize)
        {
            let _ = reader.cancel();
            return Err("Response exceeds the configured limit");
        }
        let start = bytes.len();
        bytes.resize(start + length, 0);
        chunk.copy_to(&mut bytes[start..]);
    }
    Ok(bytes)
}
type ActiveRequest = RwSignal<Option<(u32, AbortController)>, LocalStorage>;
fn dispatch(
    state: RwSignal<State>,
    message: RwSignal<String>,
    active: ActiveRequest,
    retry: RwSignal<Option<QueryRequest>>,
    effect: Option<Effect>,
) {
    match effect {
        Some(Effect::Request {
            generation,
            request,
        }) => {
            let mut fresh = (*request).clone();
            // Restart a cancelled page from the first page at its pinned snapshot.
            fresh.cursor = None;
            retry.set(Some(fresh));
            let controller = match AbortController::new() {
                Ok(value) => value,
                Err(_) => {
                    state.update(|s| {
                        let _ = s.transition(Event::Disconnected);
                    });
                    message.set("Could not start the query.".into());
                    return;
                }
            };
            // Register synchronously, before the asynchronous fetch can start.
            active.set(Some((generation, controller.clone())));
            spawn_local(async move {
                let result = fetch_query(*request, controller).await;
                active.update(|slot| {
                    if slot.as_ref().is_some_and(|(g, _)| *g == generation) {
                        *slot = None;
                    }
                });
                state.update(|s| {
                    if s.generation() != generation || s.phase() != Phase::Loading { return; }
                    match result {
                        Ok(bytes) => match s.transition(Event::Response { generation, bytes }) {
                            Ok(_) => message.set(String::new()),
                            Err(_) => { let _ = s.transition(Event::Disconnected); message.set("The response did not match the requested scope, snapshot, or limits".into()); }
                        },
                        Err(error) => { let _ = s.transition(Event::Disconnected); message.set(error.into()); }
                    }
                });
            });
        }
        Some(Effect::Cancel { generation }) => {
            active.update(|slot| {
                if slot.as_ref().is_some_and(|(g, _)| *g == generation)
                    && let Some((_, controller)) = slot.take()
                {
                    controller.abort();
                }
            });
        }
        None => {}
    }
}
fn key_text(key: &RunKey) -> String {
    match key {
        RunKey::Global { run_id } => run_id.clone(),
        RunKey::Legacy {
            origin,
            producer,
            generation,
            run_id,
        } => format!("{origin} / {producer} / {generation} / {run_id}"),
    }
}
fn summary(row: RunSummary) -> impl IntoView {
    let stats = row.statistics;
    let min = stats.min_ns.map(|v| v.to_string());
    let max = stats.max_ns.map(|v| v.to_string());
    let sum = stats.sum_ns.map(|v| v.to_string());
    let mean = stats
        .mean_ns
        .map(|mean| format!("{} / {}", mean.numerator, mean.denominator));
    let duration = |value: Option<String>| value.unwrap_or_else(|| "Unavailable".into());
    view! {
        <section>
            <h3><code>{key_text(&row.run)}</code></h3>
            <dl>
                <dt>"Observations"</dt><dd>{stats.count}</dd>
                <dt>"Errors"</dt><dd>{stats.errors}</dd>
                <dt>"Cancelled"</dt><dd>{stats.cancelled}</dd>
                <dt>"Duration samples"</dt><dd>{stats.duration_count}</dd>
                <dt>"Minimum (ns)"</dt><dd>{duration(min)}</dd>
                <dt>"Maximum (ns)"</dt><dd>{duration(max)}</dd>
                <dt>"Sum (ns)"</dt><dd>{duration(sum)}</dd>
                <dt>"Exact mean (ns)"</dt><dd>{duration(mean)}</dd>
            </dl>
        </section>
    }
}
#[component]
fn App() -> impl IntoView {
    let state = RwSignal::new(State::default());
    let base = RwSignal::new(None::<QueryRequest>);
    let retry = RwSignal::new(None::<QueryRequest>);
    let active = RwSignal::new_local(None::<(u32, AbortController)>);
    let authority = RwSignal::new(String::new());
    let scope = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let detail = RwSignal::new(false);
    let start = move |_| {
        let expected = authority.get_untracked().trim().to_owned();
        let sources: Vec<String> = scope
            .get_untracked()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let request = QueryRequest {
            version: API_VERSION,
            query_id: "ui-draft".into(),
            authority: Some(expected.clone()),
            scope: sources,
            snapshot: Default::default(),
            filter: Filter::default(),
            operation: Operation::ListRuns,
            budgets: Budgets::default(),
            cursor: None,
        };
        let mut current = state.get_untracked();
        match current.transition(Event::Start {
            request: Box::new(request.clone()),
            expected_authority: expected,
        }) {
            Ok(t) => {
                base.set(Some(request));
                detail.set(false);
                message.set(String::new());
                state.set(current);
                dispatch(state, message, active, retry, t.effect);
            }
            Err(_) => {
                message.set("Enter a valid authority and comma-separated source scope".into())
            }
        }
    };
    let back = move |_| {
        let mut current = state.get_untracked();
        let Some(request) = base.get_untracked() else {
            return;
        };
        let Some(expected) = request.authority.clone() else {
            return;
        };
        match current.transition(Event::Start {
            request: Box::new(request),
            expected_authority: expected,
        }) {
            Ok(t) => {
                detail.set(false);
                message.set(String::new());
                state.set(current);
                dispatch(state, message, active, retry, t.effect);
                focus_results();
            }
            Err(_) => message.set("Could not return to runs".into()),
        }
    };
    let cancel = move |_| {
        let mut current = state.get_untracked();
        if let Ok(t) = current.transition(Event::Cancel) {
            state.set(current);
            message.set("Stopped waiting. Backend work may still continue.".into());
            dispatch(state, message, active, retry, t.effect);
            focus_results();
        }
    };
    let retry_read = move |_| {
        let Some(request) = retry.get_untracked() else {
            return;
        };
        let Some(expected_authority) = request.authority.clone() else {
            return;
        };
        let mut current = state.get_untracked();
        match current.transition(Event::Start {
            request: Box::new(request),
            expected_authority,
        }) {
            Ok(t) => {
                state.set(current);
                message.set(String::new());
                dispatch(state, message, active, retry, t.effect);
                focus_results();
            }
            Err(_) => message.set("Could not retry the query.".into()),
        }
    };
    let select = move |index: usize| {
        let mut current = state.get_untracked();
        let Some(mut request) = base.get_untracked() else {
            return;
        };
        let Some(row) = current.rows().get(index) else {
            return;
        };
        let Some(coverage) = current.coverages().first() else {
            return;
        };
        request.operation = Operation::RunSummary;
        request.filter.run = Some(row.run.clone());
        request.snapshot = coverage.snapshot.clone();
        let mut pinned_list = request.clone();
        pinned_list.operation = Operation::ListRuns;
        pinned_list.filter.run = None;
        base.set(Some(pinned_list));
        request.cursor = None;
        let Some(expected) = request.authority.clone() else {
            return;
        };
        match current.transition(Event::Start {
            request: Box::new(request),
            expected_authority: expected,
        }) {
            Ok(t) => {
                detail.set(true);
                message.set(String::new());
                state.set(current);
                dispatch(state, message, active, retry, t.effect);
                focus_results();
            }
            Err(_) => message.set("Could not start the summary query".into()),
        }
    };
    view! {
        <header><h1>"Fabric investigation"</h1></header>
        <main>
          <form on:submit=move |ev| { ev.prevent_default(); start(()); }>
            <label>"Authority" <input required prop:value=move || authority.get() on:input=move |ev| authority.set(event_target_value(&ev)) /></label>
            <label>"Sources (comma-separated)" <input required prop:value=move || scope.get() on:input=move |ev| scope.set(event_target_value(&ev)) /></label>
            <button type="submit" disabled=move || state.get().phase() == Phase::Loading>"Find runs"</button>
          </form>
          <p class="status" role="status" aria-live="polite">{move || {
              let s = state.get(); let m = message.get();
              if !m.is_empty() { m } else { match s.phase() { Phase::Idle => String::new(), Phase::Loading => "Loading…".into(), Phase::Ready if s.rows().is_empty() => "No runs returned for this scope and snapshot".into(), Phase::Ready => "Results ready".into(), Phase::Cancelled => "Stopped waiting. Backend work may still continue.".into(), Phase::Unknown => "The connection was lost. The request outcome is unknown.".into() } }
          }}</p>
          {move || (state.get().phase() == Phase::Loading).then(|| view! { <button type="button" on:click=cancel>"Stop waiting"</button> })}
          {move || matches!(state.get().phase(), Phase::Cancelled | Phase::Unknown).then(|| view! { <div><button type="button" on:click=retry_read>"Retry read"</button><p>"A retry starts a fresh read. A paged list starts again at its pinned snapshot."</p></div> })}
          {move || if detail.get() { Some(view! { <button type="button" disabled=move || state.get().phase() == Phase::Loading on:click=back>"← Back to runs"</button> }) } else { None }}
          <h2 id="results-heading" tabindex="-1">{move || if detail.get() { "Run aggregate summary" } else { "Runs" }}</h2>
          {move || if detail.get() {
            state.get().rows().first().cloned().map(summary).into_any()
          } else {
            let rows = state.get().rows().to_vec();
            view! { <ol class="ledger">{rows.into_iter().enumerate().map(|(i, row)| { let key = key_text(&row.run); let label = format!("Inspect summary for {key}"); view! { <li><code>{key}</code><button type="button" aria-label=label disabled=move || state.get().phase() == Phase::Loading on:click=move |_| select(i)>"Inspect summary"</button></li> } }).collect_view()}</ol>
            <button type="button" disabled=move || state.get().phase() != Phase::Ready || state.get().next_cursor().is_none() on:click=move |_| { let mut s = state.get_untracked(); if let Ok(t) = s.transition(Event::Next) { state.set(s); dispatch(state, message, active, retry, t.effect); } }>"Next page"</button> }.into_any()
          }}
          {move || { let coverages = state.get().coverages().to_vec(); (!coverages.is_empty()).then(|| view! { <section class="coverage"><h2>"Coverage"</h2>{coverages.into_iter().enumerate().map(|(i,c)| view! { <div><h3>{format!("Page {}", i + 1)}</h3><p>{if c.complete { "Complete" } else { "Partial. More results available." }}</p><dl><dt>"Authority"</dt><dd>{c.canonical_authority}</dd><dt>"Sources"</dt><dd>{c.scope.join(", ")}</dd><dt>"Snapshot"</dt><dd>{serde_json::to_string(&c.snapshot).unwrap_or_default()}</dd><dt>"Reported gaps"</dt><dd>{if c.observed_gaps.is_empty() { "None reported".to_owned() } else { c.observed_gaps.join(", ") }}</dd></dl></div> }).collect_view()}</section> }) }}
        </main>
    }
}
fn main() {
    leptos::mount::mount_to_body(App);
}
