const fs = require("node:fs");
(async () => {
  const {instance} = await WebAssembly.instantiate(fs.readFileSync(process.argv[2]), {});
  const length = instance.exports.fixture_len();
  const pointer = instance.exports.fixture_ptr();
  const bytes = Buffer.from(instance.exports.memory.buffer, pointer, length);
  process.stdout.write(bytes);
})().catch(error => { console.error(error); process.exitCode = 1; });
