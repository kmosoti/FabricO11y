# Visible gap counts in retained-query results

The packaged Chrome response diagnostic at
`/run/media/kmosoti/data/FabricO11y/results/replacement-tls-01-chrome-response-positive/browser.json`
(SHA-256 `742c3be811392c5fcfb12e507d4577d6ce1f4cd2849d04059c92b9690da5db82`)
failed on the injected `gap` case. It is preserved as an actual failed result;
the response was a diagnostic fixture, not a server outcome. The fixture supplied
`complete=false`, one `gaps` entry, and an empty `unavailable` array. The separate
`incomplete` case also supplied `complete=false`, without a gap.

The frozen product source `e078ba23b543126011a2136dd1f7e98ca65370eb` rendered
both cases in Explore as `200 observations · Incomplete coverage — review the
evidence`. `QueryResults` exposed the raw `gaps` field in a collapsed details
disclosure, but its visible status did not identify that explicit gap. The
existing `EvidenceSummary` already reports gap counts but is only used for the
overview process-observations view. This behavior conflicted with the product
promise to display gaps and the art-direction requirement to distinguish an
explicit gap from a partial answer.

The current-source correction appends gap and unavailable-source counts to the
existing query status while retaining the complete/incomplete text and exact
evidence disclosure. The browser response grader and its expected outcomes are
unchanged. No build or test of the correction is recorded here; the frozen
`e078ba23` package remains historical, and a new package identity and browser
rerun are required.
