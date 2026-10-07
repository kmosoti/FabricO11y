# O7 valid-Batch fixture retry

Native01 exited101 before building:64 logs of16KiB plus OTLP framing exceeded the
unchanged1MiB Batch field cap. Keep that failure and its source/binary archive.
The new fixture uses32 logs per Batch,35 Groups for the same1100 bodies and time
shape. Query committed frontier is35; producer identities follow those actual
Batches. The independent checker now requires35 producer records. No cap or
semantic assertion is relaxed; native Batch::validate must accept every Batch.

Fresh build02 uses the original build command and explicit16MiB selector.
Fresh native02 uses the original executable/driver command with output
`catalog-coupled-o7-native-02`; its20MiB global reservation and20MiB local cap
include8MiB failure reserve. Timing from failed native01 is not a performance
sample. Before new timing, remove only byte-verified archived failed scratch.
This prospective fixture/checker amendment leaves original C2 acceptance intact.
