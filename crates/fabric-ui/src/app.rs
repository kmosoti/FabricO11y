use crate::model::{
    Completion, RequestCoordinator, RowId, Sample, Signal as RecordSignal, TailBuffer, TailRow,
    TimeWindow, chart_envelope,
};
use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Explore,
    Traces,
    Pipeline,
    Tail,
    Settings,
}
impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Explore => "Explore",
            Self::Traces => "Trace detail",
            Self::Pipeline => "Pipeline",
            Self::Tail => "Live Tail",
            Self::Settings => "Settings",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Overview => "01",
            Self::Explore => "02",
            Self::Traces => "03",
            Self::Pipeline => "04",
            Self::Tail => "05",
            Self::Settings => "06",
        }
    }
}
const PAGES: [Page; 6] = [
    Page::Overview,
    Page::Explore,
    Page::Traces,
    Page::Pipeline,
    Page::Tail,
    Page::Settings,
];
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const LOGS: [(&str, &str, &str); 6] = [
    (
        "web-01",
        "1700000000000000001",
        "request started trace=4bf92f3577b34da6a3ce929d0e0e4736",
    ),
    ("web-01", "1700000000000000010", "cache lookup completed"),
    (
        "db-01",
        "1700000000000000020",
        "query completed trace=4bf92f3577b34da6a3ce929d0e0e4736",
    ),
    (
        "web-01",
        "1700000000000000030",
        "request completed status=200",
    ),
    (
        "edge-01",
        "1700000000000000040",
        "collection gap: source temporarily unavailable",
    ),
    (
        "web-01",
        "1700000000000000050",
        "untrusted fixture: <script>alert('text only')</script>",
    ),
];

#[component]
pub fn App() -> impl IntoView {
    let page = RwSignal::new(Page::Overview);
    let dark = RwSignal::new(false);
    let demo = RwSignal::new(false);
    provide_context(RwSignal::new(String::new()));
    view! {
        <div class="console" class:dark=move || dark.get()>
            <a class="skip-link" href="#main-content">"Skip to main content"</a>
            <aside class="sidebar">
                <a class="brand" href="#" on:click=move |ev| { ev.prevent_default(); page.set(Page::Overview); }><span>"Fabric"<b>"O11y"</b></span></a>
                <p class="nav-caption">"WORKSPACE"</p>
                <nav aria-label="Main navigation">{PAGES.into_iter().map(move |item| view! {
                    <button class="nav-item" class:active=move || page.get() == item aria-current=move || if page.get() == item { "page" } else { "false" } on:click=move |_| page.set(item)><span aria-hidden="true">{item.icon()}</span>{item.title()}</button>
                }).collect_view()}</nav>
                <div class="sidebar-foot"><span class="status-dot"></span>"Local workspace"<small>"Evidence before certainty"</small></div>
            </aside>
            <div class="workspace">
                <header class="topbar"><div class="breadcrumb">"Workspace"<span>" / "</span>{move || page.get().title()}</div><div class="top-actions"><span class="badge" class:warning=move || demo.get()>{move || if demo.get() { "DEMONSTRATION" } else { "CONNECTION REQUIRED" }}</span><button class="theme-button" aria-label="Toggle color theme" on:click=move |_| dark.update(|v| *v = !*v)>{move || if dark.get() { "Theme: light" } else { "Theme: dark" }}</button></div></header>
                <main id="main-content">
                    <Show when=move || demo.get() fallback=move || view! {
                        <section class="locked card" data-testid="locked-screen"><p class="eyebrow">"FABRIC O11Y OPERATOR CONSOLE"</p><h1>"Connect to a server"</h1><p>"This preview contains demonstration data only. Secure server sign-in is not available yet."</p><p class="muted">"No admin credential is requested, stored or embedded in this app."</p><button class="primary" data-testid="enter-demo" on:click=move |_| demo.set(true)>"Explore demonstration"<span>" →"</span></button><small>"Deterministic synthetic data · no network requests · no persisted telemetry"</small></section>
                    }>
                        <div class="demo-note" role="status"><span>"◇"</span>"Demonstration workspace. All observations below are synthetic fixture data, not a live deployment."<button on:click=move |_| demo.set(false)>"Exit demo"</button></div>
                        <div class="page-heading"><div><p class="eyebrow">"OBSERVE / UNDERSTAND"</p><h1>{move || page.get().title()}</h1><p class="muted">{move || match page.get() { Page::Overview => "A clear view of retained evidence and its limits.", Page::Explore => "Ask a bounded question. Keep the evidence in view.", Page::Traces => "Follow recorded spans without inventing missing context.", Page::Pipeline => "Understand where telemetry lives and who holds custody.", Page::Tail => "A bounded preview, with explicit pause and retention.", Page::Settings => "Identity, appearance and effective storage policy." }}</p></div><span class="fixture-chip">"Fixture / 01"</span></div>
                        {move || match page.get() {
                            Page::Overview => view! { <Overview navigate=page/> }.into_any(),
                            Page::Explore => view! { <Explore navigate=page/> }.into_any(),
                            Page::Traces => view! { <TraceDetail navigate=page/> }.into_any(),
                            Page::Pipeline => view! { <Pipeline/> }.into_any(),
                            Page::Tail => view! { <LiveTail/> }.into_any(),
                            Page::Settings => view! { <Settings dark=dark/> }.into_any(),
                        }}
                    </Show>
                </main>
                <footer>"FabricO11y"<span>"Fidelity · Custody · Completeness · Freshness · Boundedness"</span></footer>
            </div>
            <nav class="mobile-nav" aria-label="Mobile navigation">{PAGES.into_iter().map(move |item| view! { <button class:active=move || page.get() == item on:click=move |_| page.set(item)><span>{item.icon()}</span>{item.title()}</button> }).collect_view()}</nav>
        </div>
    }
}

#[component]
fn Evidence() -> impl IntoView {
    view! { <div class="evidence"><span class="badge warning">"Partial fixture"</span><span>"1 collection gap"</span><span>"Snapshot: demo-01"</span><span>"Fixture window: 2023-11-14 · 22:13:20–22:14:20 UTC"</span></div> }
}

#[component]
fn Overview(navigate: RwSignal<Page>) -> impl IntoView {
    view! {
        <div class="stats-grid" aria-label="Fixture inventory">
            <Stat label="Log records" value="6" note="Retained fixture rows"/>
            <Stat label="Metric samples" value="12" note="11 values · 1 explicit gap"/>
            <Stat label="Recorded spans" value="4" note="One parent is unavailable"/>
            <Stat label="Collection gaps" value="1" note="Coverage remains explicit"/>
        </div>
        <div class="two-column"><section class="card"><div class="card-heading"><h2>"Memory observations"</h2><span class="badge">"Fixture · MiB"</span></div><MetricChart/><p class="chart-caption">"Bounded synthetic samples. The gap is left visible."</p></section><section class="card"><div class="card-heading"><h2>"Source evidence"</h2><span class="badge">"3 sources"</span></div><div class="source-row"><span class="source-icon">"W"</span><div><b>"web-01"</b><small>"Logs and trace spans"</small></div><span class="badge">"Recorded"</span></div><div class="source-row"><span class="source-icon">"D"</span><div><b>"db-01"</b><small>"Related log and span"</small></div><span class="badge">"Recorded"</span></div><div class="source-row"><span class="source-icon warning">"E"</span><div><b>"edge-01"</b><small>"Source interruption reported"</small></div><span class="badge warning">"Gap"</span></div><p class="muted">"Freshness describes retained observation time, not a current heartbeat."</p></section></div>
        <section class="card"><div class="card-heading"><h2>"Investigate the evidence"</h2><button class="text-button" on:click=move |_| navigate.set(Page::Explore)>"Open Explore →"</button></div><Evidence/><div class="investigation"><div class="investigation-symbol">"≋"</div><div><b>"A request crossed two sources"</b><p>"Inspect the span waterfall and explicit missing parent, then search the matching log text."</p></div><button class="secondary" on:click=move |_| navigate.set(Page::Traces)>"View trace"</button></div></section>
    }
}

#[component]
fn Stat(label: &'static str, value: &'static str, note: &'static str) -> impl IntoView {
    view! { <section class="stat"><p>{label}</p><strong>{value}</strong><small>{note}</small></section> }
}

#[component]
fn Explore(navigate: RwSignal<Page>) -> impl IntoView {
    let kind = RwSignal::new("logs");
    let source = RwSignal::new(String::new());
    let linked_filter = use_context::<RwSignal<String>>().expect("console query context");
    let initial_filter = linked_filter.get_untracked();
    linked_filter.set(String::new());
    let filter = RwSignal::new(initial_filter.clone());
    let filter_error = RwSignal::new(false);
    let applied = RwSignal::new((String::new(), initial_filter));
    let queried = RwSignal::new(false);
    let mut coordinator = RequestCoordinator::new();
    coordinator.login().expect("fresh fixture epoch");
    let coordinator = RwSignal::new(coordinator);
    view! {
        <section class="card query-card"><div class="tabs" role="group" aria-label="Signal type">{["logs", "metrics", "spans"].into_iter().map(move |tab| view! { <button class:active=move || kind.get() == tab on:click=move |_| { kind.set(tab); queried.set(false); }>{tab}</button> }).collect_view()}</div>
            <form class="query-form" on:submit=move |ev| { ev.prevent_default(); if filter_error.get_untracked() { return; } coordinator.update(|state| {
                if state.change_query().is_ok() && let Ok(token) = state.start()
                    && state.complete(token) == Completion::Accepted {
                    applied.set((source.get(), filter.get()));
                    queried.set(true);
                }
            }); }>
                <label>"Source"<select prop:value=move || source.get() on:change=move |ev| source.set(event_target_value(&ev))><option value="">"All sources"</option><option>"web-01"</option><option>"db-01"</option><option>"edge-01"</option></select></label>
                <label class="filter-field">{move || match kind.get() { "metrics" => "Exact metric name", "spans" => "Exact trace ID", _ => "Body contains (case-sensitive)" }}<input maxlength="4096" aria-describedby="filter-help" aria-invalid=move || filter_error.get().to_string() prop:value=move || filter.get() placeholder=move || match kind.get() { "metrics" => "process.memory.mib", "spans" => TRACE, _ => "Search log text…" } on:input=move |ev| {
                    let value = event_target_value(&ev);
                    if value.len() > 4096 {
                        event_target::<web_sys::HtmlInputElement>(&ev).set_value(&filter.get_untracked());
                        filter_error.set(true);
                    } else {
                        filter.set(value);
                        filter_error.set(false);
                    }
                }/></label>
                <label>"Time window"<select><option>"Fixture minute · UTC"</option></select></label><button class="primary" type="submit" data-testid="run-query">"Run query"</button>
            </form><p id="filter-help" class="query-help" role="status">{move || if filter_error.get() { "Filter exceeds 4,096 UTF-8 bytes; edit it before running. The previous filter is retained." } else { "Filter limit: 4,096 UTF-8 bytes." }}</p><p class="query-help">"Window [1700000000000000000, 1700000060000000000) Unix ns. Supported structured filters only. No SQL, service grouping or arbitrary aggregation. Display cap: 6 rows."</p>
        </section>
        <section class="card results"><div class="card-heading"><h2>{move || format!("{} results", kind.get())}</h2><span class="badge">{move || if queried.get() { "Local fixture query" } else { "Fixture preview" }}</span></div><Evidence/>
            {move || match kind.get() {
                "metrics" => { let (node, name) = applied.get(); if (node.is_empty() || node == "web-01") && (name.is_empty() || name == "process.memory.mib") { view! { <MetricChart/><p class="chart-caption">"process.memory.mib · web-01 · no rate interpolation or server aggregate"</p> }.into_any() } else { view! { <p class="empty">"No fixture metric matches these filters."</p> }.into_any() } },
                "spans" => { let (node, id) = applied.get(); if (node.is_empty() || node == "web-01" || node == "db-01") && (id.is_empty() || id == TRACE) { view! { <div class="trace-result"><span class="source-icon">"≋"</span><div><b>"GET /checkout"</b><code>{TRACE}</code><small>{format!("{} matching span{} · {} · synthetic trace", if node == "db-01" { 1 } else if node == "web-01" { 3 } else { 4 }, if node == "db-01" { "" } else { "s" }, if node.is_empty() { "all sources" } else { &node })}</small></div><button class="secondary" on:click=move |_| navigate.set(Page::Traces)>"Inspect full fixture trace →"</button></div> }.into_any() } else { view! { <p class="empty">"No fixture trace matches these filters."</p> }.into_any() } },
                _ => view! { <div class="table-scroll"><table><caption>"Synthetic log observations · applied source and body filters · maximum 6 rows"</caption><thead><tr><th>"Observed time (Unix ns)"</th><th>"Source"</th><th>"Body"</th></tr></thead><tbody>{move || { let (node, contains) = applied.get(); let matching: Vec<_> = LOGS.into_iter().filter(|(n, _, text)| (node.is_empty() || node == *n) && text.contains(&contains)).collect(); if matching.is_empty() { view! { <tr><td colspan="3" class="empty">"No fixture logs match the applied filters."</td></tr> }.into_any() } else { matching.into_iter().map(|(node, time, text)| view! { <tr><td class="mono">{time}</td><td><span class="source-label">{node}</span></td><td>{text}</td></tr> }).collect_view().into_any() } }}</tbody></table></div> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn MetricChart() -> impl IntoView {
    const START: u64 = 1_700_000_000_000_000_000;
    let values = [
        Some(42.),
        Some(48.),
        Some(44.),
        Some(61.),
        Some(53.),
        Some(78.),
        None,
        Some(68.),
        Some(58.),
        Some(79.),
        Some(84.),
        Some(73.),
    ];
    let samples: Vec<Sample> = values
        .into_iter()
        .enumerate()
        .map(|(i, value)| Sample {
            time_ns: START + i as u64 * 5_000_000_000,
            value,
        })
        .collect();
    let envelope = chart_envelope(
        &samples,
        TimeWindow {
            start_ns: START,
            end_ns: START + 60_000_000_000,
        },
        12,
    )
    .expect("registered fixture chart");
    view! { <p class="chart-context">"process.memory.mib · web-01 · 2023-11-14 · [22:13:20, 22:14:20) UTC"</p><div class="metric-chart"><div class="chart-y"><span>"96"</span><span>"64"</span><span>"32"</span><span>"0 MiB"</span></div><svg viewBox="0 0 640 180" preserveAspectRatio="none" role="img" aria-label="Synthetic memory samples range from 42 to 84 MiB, with one explicit gap. Points are independent, without interpolation."><path class="grid-line" d="M0 20H640 M0 70H640 M0 120H640 M0 170H640"/>{envelope.into_iter().map(|bucket| {
        let gap_x = (bucket.index as f64 + 0.5) * 640. / 12.;
        let points = bucket.min.into_iter().chain(bucket.max.filter(|maximum| bucket.min != Some(*maximum))).map(|point| view! { <circle class="chart-point" cx=(point.x * 640.).to_string() cy=(170. - point.value / 96. * 150.).to_string() r="4"/> }).collect_view();
        view! { <g>{points}{bucket.has_gap.then(|| view! { <text x=gap_x.to_string() y="105" text-anchor="middle" class="chart-gap">"gap"</text> })}</g> }
    }).collect_view()}</svg></div><div class="chart-x"><span>"22:13:20"</span><span>"22:13:50"</span><span>"22:14:20 UTC"</span></div><details><summary>"View exact sample values and window"</summary><p class="mono">"Window [1700000000000000000, 1700000060000000000) Unix ns; 5 s sample spacing."</p><p class="mono">"42, 48, 44, 61, 53, 78, explicit gap, 68, 58, 79, 84, 73 MiB"</p><p>"Bucket extrema preserve selected values; no interpolation, resampling or exhaustive-coverage claim."</p></details> }
}

#[component]
fn TraceDetail(navigate: RwSignal<Page>) -> impl IntoView {
    let selected = RwSignal::new(0usize);
    let linked_filter = use_context::<RwSignal<String>>().expect("console query context");
    let spans = [
        (
            "GET /checkout",
            "web-01",
            "0–120 ms",
            "0",
            "100",
            "Root span",
        ),
        (
            "cache.lookup",
            "web-01",
            "12–36 ms",
            "10",
            "20",
            "Parent: GET /checkout",
        ),
        (
            "database.query",
            "db-01",
            "42–96 ms",
            "35",
            "45",
            "Parent: GET /checkout",
        ),
        (
            "worker.finish",
            "web-01",
            "88–112 ms",
            "73",
            "20",
            "Parent span missing from retained fixture",
        ),
    ];
    view! { <section class="card"><div class="card-heading"><div><h2>"GET /checkout"</h2><code class="trace-id">{TRACE}</code></div><span class="badge warning">"Missing parent"</span></div><Evidence/><p class="chart-context">"Full fixture trace · all sources · starts 2023-11-14T22:13:20Z · relative recorded span times"</p><div class="waterfall-header"><span>"Span / source"</span><span>"0 ms"</span><span>"60 ms"</span><span>"120 ms"</span></div><div class="waterfall">{spans.into_iter().enumerate().map(move |(index,(name,node,time,left,width,_))| view! { <button class="span-row" class:selected=move || selected.get() == index on:click=move |_| selected.set(index)><div><b>{name}</b><small>{node}</small></div><div class="span-track"><svg viewBox="0 0 100 12" preserveAspectRatio="none" aria-hidden="true"><rect class="span-rect" x=left y="1" width=width height="10" rx="0.8"/></svg><span class="span-time">{time}</span></div></button> }).collect_view()}</div><div class="span-details"><p class="eyebrow">"SELECTED SPAN"</p><h3>{move || spans[selected.get()].0}</h3><p>{move || spans[selected.get()].5}</p><p class="muted">"Timing uses recorded span timestamps. Source clocks are not independently synchronized in this fixture."</p></div><div class="card-heading"><p class="muted">"Related-log search requires an explicit trace ID in body text; absence does not prove no logs exist."</p><button class="secondary" data-testid="related-logs" on:click=move |_| { linked_filter.set(TRACE.to_owned()); navigate.set(Page::Explore); }>"Explore related logs"</button></div></section> }
}

#[component]
fn Pipeline() -> impl IntoView {
    view! { <section class="card"><div class="card-heading"><h2>"The custody path"</h2><span class="badge">"Architecture view"</span></div><p class="muted">"These stages describe the implemented pipeline. Live queue counters are unavailable until an authorized status adapter is connected."</p><div class="pipeline">{[("01", "Spindle Spool", "Durable source custody", "Unacknowledged Batches remain retained."),("02", "TLS delivery", "Authenticated transfer", "Exact identity and bytes survive retries."),("03", "Committed journal", "Server custody after sync", "ACK follows grouped two-sync commit."),("04", "Bounded sealer", "Off the commit path", "Publication precedes checkpoint and reclaim."),("05", "Parquet Segments", "Retained evidence", "Whole Segments expire under age/byte policy."),("06", "History query", "Journal + Segments", "Snapshot, gaps and completeness remain explicit.")].into_iter().map(|(num,title,sub,body)| view! { <article class="pipeline-stage"><span class="stage-number">{num}</span><div><h3>{title}</h3><small>{sub}</small><p>{body}</p></div><span class="stage-status">"Defined"</span></article> }).collect_view()}</div></section><section class="card"><h2>"A counter needs a definition"</h2><p class="muted">"Accepted Batches, committed records, projected rows, bytes and ACKed custody are different measurements. No event-rate or latency claim is manufactured from the visual reference."</p></section> }
}

#[component]
fn LiveTail() -> impl IntoView {
    let paused = RwSignal::new(false);
    let step = RwSignal::new(0usize);
    let rows = RwSignal::new(TailBuffer::new(6, 4_096).expect("fixed tail budget"));
    let advance = move || {
        let index = step.get_untracked();
        let (node, time, text) = LOGS[index % LOGS.len()];
        rows.update(|buffer| {
            buffer.push(TailRow {
                id: RowId {
                    enrollment: [1; 16],
                    generation: 1,
                    batch_sequence: index as u64,
                    ordinal: 0,
                    signal: RecordSignal::Logs,
                },
                text: format!("{time}  {node}  {text}"),
            });
        });
        step.update(|v| *v = v.saturating_add(1));
    };
    advance();
    view! { <section class="card"><div class="card-heading"><div><h2>"Bounded fixture tail"</h2><p class="muted">"Advance deterministic rows manually. This build has no live polling transport."</p></div><div class="button-group"><button class="secondary" data-testid="tail-pause" on:click=move |_| paused.update(|v| *v = !*v)>{move || if paused.get() { "Resume" } else { "Pause" }}</button><button class="primary" data-testid="tail-advance" disabled=move || paused.get() on:click=move |_| advance()>"Next fixture row"</button></div></div><div class="evidence"><span class="badge">{move || if paused.get() { "Paused" } else { "Manual advance" }}</span><span>"Cap: 6 rows / 4096 UTF-8 bytes"</span><span>{move || rows.with(|buffer| format!("{} bytes · {} evicted", buffer.utf8_bytes(), buffer.dropped().count))}</span><span>"No delivery-completeness claim"</span></div><div class="tail-terminal" role="log" aria-label="Synthetic log tail">{move || rows.with(|buffer| buffer.rows().map(|row| view! { <div class="tail-line">{row.text.clone()}</div> }).collect_view())}</div></section> }
}

#[component]
fn Settings(dark: RwSignal<bool>) -> impl IntoView {
    view! { <div class="two-column"><section class="card"><p class="eyebrow">"IDENTITY & ACCESS"</p><h2>"Production session required"</h2><p class="muted">"Passkey enrollment, recovery, session revocation and scoped Spindle enrollment/configuration/pause/revoke are release requirements. They are unavailable in this demonstration foundation."</p><span class="badge warning">"Not implemented"</span></section><section class="card"><p class="eyebrow">"APPEARANCE"</p><h2>"Make room for your work"</h2><p class="muted">"The theme is held in page memory only."</p><button class="secondary" on:click=move |_| dark.update(|v| *v = !*v)>{move || if dark.get() { "Use light theme" } else { "Use dark theme" }}</button></section></div><section class="card"><div class="card-heading"><h2>"Retention & storage policy"</h2><span class="badge">"Read-only plan"</span></div><div class="policy-grid"><div><small>"Release requirement"</small><strong>"100 GB"</strong><p>"Decimal sealed-Segment retention default"</p></div><div><small>"Age retention"</small><strong>"24 hours"</strong><p>"Either byte or age expiry can apply first"</p></div><div><small>"Custody boundaries"</small><strong>"Separate budgets"</strong><p>"Journal, workspace and edge Spools need headroom"</p></div></div><p class="muted">"This is intended release policy, not a fetched effective configuration. Current server defaults remain 20 GiB. Operators use the protected server configuration file and documented restart procedure; no browser write is offered."</p></section> }
}
