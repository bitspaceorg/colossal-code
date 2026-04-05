# Agent Protocol crate

This crate implements the Agent-to-Agent (A2A) protocol, including the JSON-RPC
schema that agents speak when exchanging tasks and messages.

## JSON-RPC Send Message Parameters

The `message/send` and `message/stream` requests include a `params` payload that
uses the `SendMessageParams` structure. Alongside the existing `message` and
`configuration` fields, the payload now supports an optional `specStep`
attribute:

```json
{
    "message": {
        /* ... */
    },
    "specStep": {
        "specId": "spec-123",
        "index": "2",
        "instructions": "Implement the parser"
    },
    "configuration": {
        /* optional overrides */
    }
}
```

`specStep` serializes the `SpecStepRef` type and lets integrations indicate the
specific step within a `SpecSheet` that a message is addressing. The field is
optional (`None` by default), preserving backwards compatibility for existing
clients that omit it.
