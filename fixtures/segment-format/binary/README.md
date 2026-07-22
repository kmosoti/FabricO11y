# Sealed byte fixtures

Each `*.hex` file is the lowercase hexadecimal form of one complete format-v1 container. Regenerate
them with:

```text
cargo run -p fabric-segment --example generate-fixtures -- fixtures/segment-format/binary
```

`valid.hex` decodes without a dictionary. `corrupt-payload.hex` changes one compressed byte without
changing its trailer digest. `truncated.hex` removes the last trailer byte.
`missing-dictionary.hex` is valid but references an immutable dictionary not supplied to the
decoder. Rust conformance tests assert the stable category for every case.
