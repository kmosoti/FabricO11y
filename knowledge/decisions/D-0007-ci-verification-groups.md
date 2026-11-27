# CI verification groups

The development workflow ran the workspace test, lint, and schema checks in
both parallel required jobs. Scheduled execution also repeated the common
checks in the extensive tier. The workflow now assigns each group once
while both required checks still gate integration.

Native coverage tests remain separate executions when they supply exact test
selector evidence. Local candidate verification still runs full acceptance
under one candidate identity. The split does not rely on cached test outcomes.
