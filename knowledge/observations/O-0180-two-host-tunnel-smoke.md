---
id: O-0180
date: 2026-11-18
added_by: agent
human_ratified: false
---

# Two physical origin hosts over authenticated tunnels

A preliminary #180 smoke test connected two DigitalOcean Linux hosts to the
receiver on fedora-blade through independent authenticated SSH reverse tunnels.
The receiver and both temporary Spindle services ran under scoped process and
resource limits. The host scratch directories were on persistent ext4.

Both origins committed source ID 1 before the receiver was available. After
reconnection the receiver reported separate authority-scoped frontiers through
ID 1, an explicit typed query returned two rows with that same source ID and
distinct origins, and the credential for one origin could not read the other.
The trial stopped its own services and closed its tunnels when finished.

The copied Spindle executable, with debug information stripped, had SHA-256
`21177105174bb889c0be797f890eba2fbc85b4039d89561b0cb5421c263bd59a`.
The local receiver executable had SHA-256
`5f008bea609a42beec6ce87636211230a2467084febbf929fb275ee93031eb03`.
This trial used the available receiver binary and preceded assembly of the
full candidate. It established real cross-host forwarding and authorization.
The physical trial did not exercise receiver self-telemetry or compare
performance.
