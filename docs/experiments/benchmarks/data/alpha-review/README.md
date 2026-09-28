# Frozen alpha review evidence

Copied unmodified on 2026-09-27 from ignored `target/alpha-*` directories, which `cargo clean` would delete. Reviewer Markdown reports and parent notes carry a `.txt` suffix because their links were written for their original locations. Probe manifests still reference those original paths and are not built from here; rerun a probe by copying it back beside a current build. The [phase-1 checkpoint](../../../formal/alpha-phase1-review-checkpoint.md) and [phase-0 review response](../../alpha-phase0-review-response.md) explain what each probe established. Bulk generated data, build output and the original failing journal fixtures stay under `target/`.

| Directory | Original location |
| --- | --- |
| `phase0-harness-gpt/` | `target/alpha-harness-review-gpt/ (GPT phase-0 harness probe and verdict)` |
| `phase1-gpt-api/` | `target/alpha-phase1-gpt-api/ (GPT public Config probe, Rust)` |
| `phase1-gpt-followup/` | `target/alpha-phase1-gpt-followup/ (GPT reuse/gaps/inspect probes on original and repaired binaries; pre-repair source snapshot)` |
| `phase1-log-review/` | `target/alpha-log-review/ (independent log-reader probe, original)` |
| `phase1-log-review-repaired/` | `target/alpha-log-review-repaired/ (same probe with mechanical cursor adapter)` |
| `phase1-journal-oracle/` | `target/alpha-journal-oracle/ (independent journal oracle, frozen)` |
| `phase1-journal-oracle-adapted/` | `target/alpha-journal-oracle-adapted/ (same oracle with borrowed-append and cursor adapters)` |
| `parent-notes/` | `target/alpha-implementation/ (parent briefs, steering, reviewer reports and verdict JSON)` |

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `parent-notes/brief.md.txt` | 10778 | `53a06fa510c2ed0d153fe2e33aefb8f64a16ce5c1008e7343a455d3785eaf2bb` |
| `parent-notes/checkpoint-brief.md.txt` | 2539 | `2f6fab1597e69a227e2b9919e5081afca4c5c1f32020772f8b32d6e614f2143d` |
| `parent-notes/checkpoint-last.md.txt` | 1571 | `8312d206d895b974fb205acc3d896f6f77c7d7f81f709256ee09a134b46fad2b` |
| `parent-notes/checkpoint-unicode-final.json` | 894 | `d9fa3ae1f99d15f0275ffa0c7e2553899836d0cb1dee7088228d174e65c35896` |
| `parent-notes/claude-checkpoint-review.json` | 7218 | `1e1fb82b5e5be4b68b41985a74de0053c6fb57f2db9bb2c732b35ffd8a1c4a36` |
| `parent-notes/claude-checkpoint-review.md.txt` | 2086 | `090b5bba66ec2b6b37c2898570a2dcf5f7481f839aef7a5189c378346430085b` |
| `parent-notes/claude-phase0-brief.md.txt` | 2113 | `13a6dc6be264772afeb7bc2e99de5d41115d7c68bb269a80a53eb66d4f1abe5c` |
| `parent-notes/claude-phase0-rereview.json` | 11770 | `67f1e11feb7fa37f59c1395b89a3ad1f06a10831a1f8e92658eb1b86f2a4ffdd` |
| `parent-notes/claude-phase0-rereview.md.txt` | 2690 | `60ea743c560b123820f5e6119df240ef9942b1e0755b9d6ce8020b5307d96117` |
| `parent-notes/claude-phase0.json` | 14204 | `8ef44f87c03704bb2d57f21efef15d951a5bab6ad352d4b896c9884e450ad182` |
| `parent-notes/claude-phase1-review.json` | 9525 | `a0e7978e8091fe7c26489a76f6b7d10dff850e1e7ad13daab0787e39ce11e802` |
| `parent-notes/claude-phase1-review.md.txt` | 3943 | `832fe1cd92ab3a75eccb7ee826a1b493d262a1a375dfb21c5f451dfa756386b8` |
| `parent-notes/journal-corruption-matrix-last.md.txt` | 193 | `5acce1db95efc9228bf1e68ce3fab7096cc5af84a0f9ba6b9a8f5f678287c3db` |
| `parent-notes/journal-corruption-matrix.md.txt` | 1346 | `03acd56222cecc19143ca1c8231b022fa4314b807040d30f22d9ceb2ca2612ec` |
| `parent-notes/journal-fault-oracle-last.md.txt` | 1257 | `4ccb72022dfaa7c82b89656cdaa99d4bac9b8f1a08df96db5dc0b628ed6fda16` |
| `parent-notes/journal-fault-oracle.md.txt` | 2411 | `e2170b261c8927db768ad4230741af80576329c464156b1cd6f4b92b0ceffd03` |
| `parent-notes/journal-oracle-brief.md.txt` | 4408 | `dc567bf820975a99107e7bd809a92d6e582b1acdd38ba96febc973fde377eb8c` |
| `parent-notes/journal-oracle-last.md.txt` | 1121 | `22c20c66664e406b40bd32b2319666987507e5c0f6defda71e28fac61419d238` |
| `parent-notes/linux-contract-last.md.txt` | 1022 | `ef06730a5d93e8aa2cb08fd83ddfc25ab1c5c499c425a7a5f4a23962b2b49f46` |
| `parent-notes/linux-host-steering.md.txt` | 6680 | `ce6f87199704dfb62b9cffe39c9f9c90c5b7092d6497959eee64ad3a8bcdecd7` |
| `parent-notes/native-reconciliation.json` | 2740 | `a6a7b9f9969d9282feba53a6fed41824132908b71a75c60c3ebf93e5077d88ed` |
| `parent-notes/phase0-approved.json` | 1309 | `61226e6c05985f090fe021f2c5672f371b18759138e55c3f6baf44d37c1dcedf` |
| `parent-notes/phase0-last.md.txt` | 1203 | `be1e9a561a5f55778a8167f6861f168d66a197387708c1b4b209ac91038617be` |
| `parent-notes/phase0-repair.md.txt` | 2431 | `efb4c198d08e3aaae1adccdb1257e12b24d6725fa4f58c9751641d50dcf1d877` |
| `parent-notes/phase0-review-repairs.md.txt` | 3456 | `7b7acb8a1f287fa427304f93eb9d866083d9313c008ad7cb1cbbd7f882dc4612` |
| `parent-notes/phase1-final-review-feedback.md.txt` | 3476 | `5de8d4036b818a0acaa7be1318fceb7e05e19ffac7e5c1002abd90cd2145345d` |
| `parent-notes/phase1-log-feedback.md.txt` | 3121 | `68b50cf434f47d7679878d0a6ac0df192a957e0430b4a65279e152f9c5ff63db` |
| `parent-notes/phase1-review-notes.md.txt` | 2988 | `fa82f354ae002c55ddca7c641e68163fcddb9444b02a3e915abed6d81fe468f8` |
| `parent-notes/phase1.md.txt` | 5652 | `8fc246e568a8737d52c690f1825fc52cf70b1a4ebbf92f4302662facd18b8aa9` |
| `parent-notes/phase2-oracle-brief.md.txt` | 2977 | `0c0411a233242dfca89c70afc6e53e92230be4ce7e348fd090d5156e2c5080a7` |
| `parent-notes/runner-review.json` | 1306 | `46350a5f8370993130c562bb0edc9146d5921fb938e095c7c8e4cfb4eca0f6ca` |
| `phase0-harness-gpt/final-extra.json` | 291 | `f5b8293a2c01e9012a95b50e9d8accf6dbffc1d217b8a00441c4aefa05a31ff8` |
| `phase0-harness-gpt/probe.py` | 9753 | `cdc1d7fa7087e0de7f60e88912ddf2739831c6a26a3ac17e484d2a479fc678a0` |
| `phase0-harness-gpt/rate_child.py` | 1465 | `9f0b99f78fc4a771e2b2d718f2af08f56cd913062f105bb45a17fe2fb3597433` |
| `phase0-harness-gpt/results.json` | 9061 | `a4424dac504722282b7d694000c94f8352a6fbd38a3d6d8d94330ef65d6824b7` |
| `phase0-harness-gpt/review-summary.json` | 1606 | `c3e24d925f7944f85467ecb2e9125cb505ebae0ef001605aaf6342364961057b` |
| `phase1-gpt-api/Cargo.toml` | 127 | `29630e06d2d57b51ee8c2f526ade32552d836d60b8373d251392b27bc26326c4` |
| `phase1-gpt-api/results.json` | 4378 | `3ddea105a56dd34420700c303c93866f7d35201f5239ae6258056bac1fe1aacb` |
| `phase1-gpt-api/src/main.rs` | 1532 | `88a1c7d6a2c01c45bbf959062e63cb7f4782f64bdb050e038f5f2656147ff627` |
| `phase1-gpt-followup/gaps-result.json` | 2233 | `c169ec0dd2b286d90ca53095139b7cd361dee04bf44fb82d05b9859e0204a0c1` |
| `phase1-gpt-followup/inspect-result.json` | 2009 | `908d7df79414ea385c45488a992799a16addebfa20469b44ca5e2281c22cf874` |
| `phase1-gpt-followup/probe.py` | 6986 | `3255f7c695da40368c8b1fd47a89a9f297a4529188de532f20f508ee47a8d029` |
| `phase1-gpt-followup/repaired-gaps-result.json` | 2313 | `b379e85b0e2789b5ad1dde8c6ec54e2377f2976075f858a3d22ebcc46a8ef7d0` |
| `phase1-gpt-followup/repaired-inspect-result.json` | 1374 | `ea792f98ff81b2f71b51b1f4f8e43f8c75695590dd9395df249097489fc10fb0` |
| `phase1-gpt-followup/repaired-reuse-result.json` | 1949 | `8c9ff5836ffda5fe62a94d6b8590c36b703bc0b2cde018beb7efa7d502020b7e` |
| `phase1-gpt-followup/repaired-review-summary.json` | 2494 | `02cad49d84d8877af6c844d16a6c832d757470671dcdb0efb91f1da6ed88ea38` |
| `phase1-gpt-followup/repaired_probe.py` | 8247 | `dd3a920f5dd30777d1782d6bc4afbb582c393f548a02eb9c16618b5a667b7dc8` |
| `phase1-gpt-followup/reuse-result.json` | 1878 | `1d59dac6a4acb4b8124d636a5f60280db9879c5bca554db35cb78a1120af4c23` |
| `phase1-gpt-followup/pre-repair-source/journal.rs` | 19171 | `cca0d42a16dafb0edcdc5bb1610e03be9553e72909fba36adcd717f28f6762b7` |
| `phase1-gpt-followup/pre-repair-source/log_source.rs` | 4077 | `9f28eb01da68f9d45b5a6d8a0eda6d119ab3684ac9cb1ce7e924bf683e95f76b` |
| `phase1-gpt-followup/pre-repair-source/node.rs` | 22409 | `94f081934bbd23ecd6d17a32ac38a479c5725618b62aea65084c65e937c87b45` |
| `phase1-journal-oracle/CORRUPTION-ASSUMPTIONS.md.txt` | 3235 | `27f77737d801c95b5f0751ca16ef57623efe095ccc03888ec463be0ceb6d2dcd` |
| `phase1-journal-oracle/CORRUPTION-RESULTS.md.txt` | 4020 | `442ff126a9c458aab4e82e9666595e43f38ba69f23b959deb155d2ada3140f13` |
| `phase1-journal-oracle/Cargo.toml` | 269 | `6a09dd3bdcc1e3063f2229ccac437e093b9bc9d2f95d02de6b1cc89f508617d4` |
| `phase1-journal-oracle/LENGTH-MATRIX-ASSUMPTIONS.md.txt` | 3522 | `2780797333214ba8c76c02e7aded81d7d020c034c6592b4b0b4065f9f4416f49` |
| `phase1-journal-oracle/LENGTH-MATRIX-RESULTS.md.txt` | 3948 | `9beee020c2fbac76e8eb73afe8ecae8018b87b8ee37575dbdb85204c7fd6d8ca` |
| `phase1-journal-oracle/README.md.txt` | 5455 | `ffd74bd6e6cd233bf0de25dc17c496102d590b4096b0a10080b8224d49b2473a` |
| `phase1-journal-oracle/RESULTS.md.txt` | 2329 | `4d315bd0409d933084a2a56109d636f0a349e3593d7acc97a713bbc0f70c8797` |
| `phase1-journal-oracle/tests/corruption_reopen.rs` | 16246 | `753cc8afbe19e64086a05956a07702d05a2556bf37ba35661e28e8d9826ddc2a` |
| `phase1-journal-oracle/tests/journal_contract.rs` | 9712 | `9a3383c49f8b6490f9c6acfa06dfa6b4c6085843eb88afdc77d672364b6bceb2` |
| `phase1-journal-oracle/tests/length_matrix_replay.rs` | 18201 | `95fe5608d9a0a4f1bd319153eb3fcb86229b120e1070e5af7f8fafb06952d504` |
| `phase1-journal-oracle-adapted/CORRUPTION-ASSUMPTIONS.md.txt` | 3235 | `27f77737d801c95b5f0751ca16ef57623efe095ccc03888ec463be0ceb6d2dcd` |
| `phase1-journal-oracle-adapted/CORRUPTION-RESULTS.md.txt` | 4020 | `442ff126a9c458aab4e82e9666595e43f38ba69f23b959deb155d2ada3140f13` |
| `phase1-journal-oracle-adapted/Cargo.toml` | 269 | `6a09dd3bdcc1e3063f2229ccac437e093b9bc9d2f95d02de6b1cc89f508617d4` |
| `phase1-journal-oracle-adapted/LENGTH-MATRIX-ASSUMPTIONS.md.txt` | 3522 | `2780797333214ba8c76c02e7aded81d7d020c034c6592b4b0b4065f9f4416f49` |
| `phase1-journal-oracle-adapted/LENGTH-MATRIX-RESULTS.md.txt` | 3948 | `9beee020c2fbac76e8eb73afe8ecae8018b87b8ee37575dbdb85204c7fd6d8ca` |
| `phase1-journal-oracle-adapted/README.md.txt` | 5455 | `ffd74bd6e6cd233bf0de25dc17c496102d590b4096b0a10080b8224d49b2473a` |
| `phase1-journal-oracle-adapted/RESULTS.md.txt` | 2329 | `4d315bd0409d933084a2a56109d636f0a349e3593d7acc97a713bbc0f70c8797` |
| `phase1-journal-oracle-adapted/tests/corruption_reopen.rs` | 16340 | `cd8074fd0e0f1f2483508d3136594a2d9aff8e02a4605aa8f6db44ec4a426b12` |
| `phase1-journal-oracle-adapted/tests/journal_contract.rs` | 9796 | `892ad84be8f6f9a4dd9f92b30b7e5fa26c02e9e94f053f0f740f165b56ed89d7` |
| `phase1-journal-oracle-adapted/tests/length_matrix_replay.rs` | 18295 | `3a11dc8929da0c7928bbad4d25c2bee57f466d0376317da691fed3f95a32e3de` |
| `phase1-log-review/Cargo.toml` | 112 | `5d37cd93a73adb446d96a2c3538b3516018394d97ee232eaf910da9898090507` |
| `phase1-log-review/repaired-results.json` | 2304 | `13e87e67d0c497cbdd81083584005e97e4cc469ea0a180ae30032c583c2d4daf` |
| `phase1-log-review/results.json` | 1566 | `a52334d5267e4ab9aa299abdf9a26c11595bd16a9a847801386978ead38dcd44` |
| `phase1-log-review/src/main.rs` | 3468 | `fa6bedc9db49271115062917f1f56476f5120c4cd726c41c0c24c1570432da29` |
| `phase1-log-review-repaired/Cargo.toml` | 133 | `fae7dfbbd2c9ca26aff1ee0965eb9ac67dc661a4f0e44b0b09f10affbede80cf` |
| `phase1-log-review-repaired/results.json` | 1566 | `a52334d5267e4ab9aa299abdf9a26c11595bd16a9a847801386978ead38dcd44` |
| `phase1-log-review-repaired/src/main.rs` | 2773 | `4c7d56b5a03f1177aa71fcf91ba2f5d15c97423a6d545da93d51124091cf9400` |
