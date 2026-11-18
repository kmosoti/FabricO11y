---
id: D-0181
title: Versioned TOML configuration for observability services
type: decision
status: accepted
date: 2026-11-18
added_by: agent
human_ratified: false
related:
  - D-0010
---

# Versioned TOML configuration for observability services

Fabric, Spindle, and Fabric Server share one bounded configuration file loader.
A TOML document declares its version and then uses the service's executable
settings schema. Explicit legacy JSON launch paths remain usable, including
the paths written by the installer.

Collector settings take precedence per field. The same resolution logic
supplies the daemon and its read-only settings report. Service discovery
prefers TOML. If a chosen file is invalid, it surfaces the error so a stale
JSON receiver or credential reference cannot silently replace it. Reports
include credential file references and keep credentials in their existing
private files.
