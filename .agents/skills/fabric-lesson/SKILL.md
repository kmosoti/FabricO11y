---
name: fabric-lesson
description: Build or explain one requested learning increment in Fabric O11y, connecting a Rust concept to an explicit system contract and a runnable example. Use for guided project work or follow-along explanations.
---

# Deliver one learning increment

Read [current state](../../../docs/CURRENT.md), the relevant [learning-path stage](../../../docs/LEARNING_PATH.md), and the implementation before choosing the increment. Honor the user's requested scope; an explanation request should produce an explanation, and a build request should produce the bounded change.

Introduce the concrete question first. Explain the data entering and leaving the component, who owns it, what can fail, and the Rust concept that makes the implementation work. Use the existing types and stable [glossary](../../../docs/glossary.md). State the property or observation that would show the design is wrong.

Build the smallest runnable increment that fulfills the request. Favor explicit code that the learner can trace; introduce a dependency or abstraction when the current contract needs it. Give a short command to run and describe its expected behavior. Add appropriate checks for new behavior, and report the commands actually executed rather than presenting examples as results.

Explain one representative path through the changed source with relative links. Update the learning stage and affected architecture under the [repository policy](../../../AGENTS.md) and [documentation policy](../../../docs/documentation-policy.md). End with what the learner can now explain, what remains uncertain, and the next proposed step. Complete authorized work without repeatedly asking permission for ordinary implementation choices.
