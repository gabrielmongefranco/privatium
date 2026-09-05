# Vendored Noble cryptography

Noble curves, ciphers, and hashes **2.4.0**, MIT licensed. Vendored 2026-09-05.

Upstream: [curves](https://github.com/paulmillr/noble-curves), [ciphers](https://github.com/paulmillr/noble-ciphers), [hashes](https://github.com/paulmillr/noble-hashes).

Files come from the versioned npm archives. Each archive is verified against its npm SHA-512 integrity value. Each package retains its complete `LICENSE`.

Only bare `@noble/hashes/...` import specifiers become relative paths into the sibling `hashes/` directory, so browsers resolve the modules without a bundler or import map. Cryptographic code is unchanged. This exception is approved in `docs/plans/phase-2.md §3`.

Upstream whitespace is preserved. Git's whitespace check reports 25 trailing-whitespace lines in these modules; trimming them would exceed the import-path-only exception.

The dependency closure starts at `curves/ed25519.js`, `ciphers/chacha.js`, `hashes/sha2.js`, and `hashes/hkdf.js`. No build step or runtime package installation is required.

| Package | Archive | SHA-512 integrity |
|---|---|---|
| `@noble/curves` 2.4.0 | [npm archive](https://registry.npmjs.org/@noble/curves/-/curves-2.4.0.tgz) | `sha512-P4/62zrgfH33CneE3Dn4WhJVA22YUU0eR51wKIan4NVRvwsA0YnPTwWGpNbpuacSujmSFLvyzpyuR30+fbq2Ew==` |
| `@noble/ciphers` 2.4.0 | [npm archive](https://registry.npmjs.org/@noble/ciphers/-/ciphers-2.4.0.tgz) | `sha512-AnjFn0Jv92laAkvMrghlFZq4qQCIN/4DxFV/eooqtC2YTjB7kBeLMS2T9KJX4Dn+ZVXLOwK0lSgqDtx9gvxtiw==` |
| `@noble/hashes` 2.4.0 | [npm archive](https://registry.npmjs.org/@noble/hashes/-/hashes-2.4.0.tgz) | `sha512-X5XaVWZIBCT7HHZGm5I7ZQXDwLG+bGXuSrMQAW+7Zvl87h1kmc1ZB1VSRJcpUfoUrGQp4Fkoxm5kZ+Ms+aW+eA==` |

| File | Upstream SHA-256 | Vendored SHA-256 |
|---|---|---|
| `ciphers/_arx.js` | `ead142ba410404c0698bb80e0d6f77780799bb77a9e3b098cf0f9fca1c486e37` | `697a8bd879939d22ea32d037a940b15781508ca59d0e85eee8d28b6d24e902e2` |
| `ciphers/_poly1305.js` | `5d2afd73b40dbafb7b6740c6e6388e7123c79a4e57306861b28c5734925fa84d` | `eca7dff9ac942e66234d6aa7718a182b4ecdcce3e8ae2365dc6e6fe985fb4d30` |
| `ciphers/_polyval.js` | `735b078133e0c5a97dd984a21389f152dc7958b283f52504f6f0906ded243fa5` | `00e64254f05044fb688b18b8ea2d4b17f36b7ab5fef35a909bd51b479c038562` |
| `ciphers/aes.js` | `941ebefe14c48cd1401ffe52f329d4e6ff14f3f596681b211a33cde21de3a90c` | `a44405b0ea0a621bc8808406af5f444b99e4305eddd9e3c5f5d84d5ddd21bc3b` |
| `ciphers/chacha.js` | `5f1c00575e227b75163f4bac50b79442dec50ee3047c46c18887808ba8af0a69` | `179d3169d1eb345b8988e7d534eb8175896a7a1ceba3cb124375d7ef54c049d8` |
| `ciphers/utils.js` | `a443ec1f60d1aaa25d1b7d8c102d33c0e8e6d683ada1ad17be9c32e69170ff95` | `e739eeb69b850b3ccf306c6449d88d27ede35dee053103a48cf05ad2c6a4fef6` |
| `curves/abstract/curve.js` | `dbaeee3b41ff47efb76b78e14170fe4dda7c7ecdc387c402f16c5118e0bac356` | `153bf617c6b1d266b6591ba42e46460ca4276010a788adff203976698fe6dbbf` |
| `curves/abstract/der.js` | `7f80d698611b131368ecf732b8740bb97f72cd42fef77c4b0bb923e2e2d44159` | `7f80d698611b131368ecf732b8740bb97f72cd42fef77c4b0bb923e2e2d44159` |
| `curves/abstract/edwards.js` | `f24a9c221a549a8fff259eb8f245ab0dc17679b13c532c3970d91acc4df9fffd` | `0e40ef7e09c6b4a9e8ee82b75b9ea4d7f0b7e4bc61007de053d8f2c870466229` |
| `curves/abstract/fft.js` | `a4b2ff7ca33f4acc85d61f0e83ec3303c3f6b38ecc19b20d0f6462d820e518cf` | `40d2659f2e630ea1d499cde2e4a166a572a35d2191c6a589c672ac92f463ea06` |
| `curves/abstract/frost.js` | `9a95f1fdf7e17b9d93a16049db675ee2967ed2a04cf59b1bd981d4662acbedf5` | `378691b2197a6e4968246f655a752894be20b1642b3bfe78d1082c3916d74b73` |
| `curves/abstract/hash-to-curve.js` | `e3c97e7d0827b728b14e3311ed11d8e386d8a70d8fc77e0b9fd2a3090f19d955` | `c4794a593c0e6b448593c293c91afa40c4a5d9d9a9b50a27df0e5ff7acdf486a` |
| `curves/abstract/modular.js` | `9ced3aa10598277a735e54f7b61d88e929d565da884ba5385f9006ebb3b6aab2` | `3fd76af533a3c544a4f39f51e39a48e2d118ce5a28accdaf2aecc2b5f838fd71` |
| `curves/abstract/montgomery.js` | `cdafa8816dad5a24475ec51952c5f71fdd5d1b46880ab982694c4f8ff605fc46` | `987df16e8d8bb9fb584b5b1d603162d820141a3101634463137b91c4293530b3` |
| `curves/abstract/oprf.js` | `2ca36b3e4930092db1f91559410e86975816c036e248067bc65a8055aa1c504c` | `58835db56426849c3e4b8af70eb7c55b2b6cd1afdfdefb8d8ade9655901d95f0` |
| `curves/abstract/weierstrass.js` | `d007e949c0d5b912e8c54132983219c8ff8f69bda058a70563b5cc7a8783331c` | `9b83dfcd4cd529ac2e896dcf903a9b0442a32df42bd3f41d3993ff079251510c` |
| `curves/ed25519.js` | `e13f6c50c36feb0d18bf7986d5f881bcefb5f1f5b16de3cc5ee7693254c7a0b5` | `f48bb4f67497d6d276e2a4cc8e07c20338557cabaf7ce3ce5e97901ef4379336` |
| `curves/misc.js` | `2db54e6ade644c4c1371903b7c88e2b77093fd3b1012d42b0a18258d1c09096d` | `4721fa2d9713f6533e13eed3b7cd01c5033e2f9c22f56ad269cad8204af11443` |
| `curves/nist.js` | `0a692b98e22c56c0354f78eb1759cfbf6aec89eafc78cd2984a4f2e8428081e6` | `04f2f3fd4f4fe68265446acd7142a98cc72db575560c7543115e508c96c086a9` |
| `curves/utils.js` | `be9ff86aef76419376e4d66cf01a134f3117f1f8dd62d81f1b79100145503367` | `8693d78c178b5c43abe2b5889a7710f920697089c0556f4b5035e5c250229828` |
| `hashes/_blake.js` | `cabbff61d1f373bac45594ad758df7c42e176464ddbb94b44f5533fa977a4f56` | `cabbff61d1f373bac45594ad758df7c42e176464ddbb94b44f5533fa977a4f56` |
| `hashes/_md.js` | `60cf3010fda89e3e4d3f0e7ff1ce249e9c467f34b4fd41b6fd6101d9f69be763` | `ba1517585e56ad61924a4842239d894c7812facded6482ec55cdb828567dc422` |
| `hashes/_u64.js` | `b09da8c07fe8187c07649494cdb7cd0bcf13df90b506a9473d19e4d5f8c2e102` | `b09da8c07fe8187c07649494cdb7cd0bcf13df90b506a9473d19e4d5f8c2e102` |
| `hashes/blake1.js` | `da1d08b4d0702ad4f185a796df8fce3fec2855de53b02190eba3bd8555677b5c` | `da1d08b4d0702ad4f185a796df8fce3fec2855de53b02190eba3bd8555677b5c` |
| `hashes/blake2.js` | `03f00237bcc0b4d822a80f7c0ac22e42632c48e246c3b6da584e6452186e32f0` | `ffc4b84cb0ebb1b84cfcaba9c39ae03b07338e459733e96df129b278f7adc48b` |
| `hashes/hkdf.js` | `ccb942a8008f974018965eeb1e33b8f4739bf767c6f95f2ceb06f5873a302f67` | `905b91ca202f3553ac6eb1d27285d5e34628ad3d7f9e2e995de7dfc769607075` |
| `hashes/hmac.js` | `fec9f8f3aeda785ea2de91cc4c059d54ef98196261c026c43f48341007ae52c9` | `fec9f8f3aeda785ea2de91cc4c059d54ef98196261c026c43f48341007ae52c9` |
| `hashes/legacy.js` | `690553a41af2fdd1e32ff6b534ba8508e95f1861c09c2d62fe762b6b501e09be` | `690553a41af2fdd1e32ff6b534ba8508e95f1861c09c2d62fe762b6b501e09be` |
| `hashes/sha2.js` | `471746bba6ec4c6238ca41358d1d3b40b6ff31cf3363f0b4d550c649c1a8e83b` | `471746bba6ec4c6238ca41358d1d3b40b6ff31cf3363f0b4d550c649c1a8e83b` |
| `hashes/sha3.js` | `9a81e1edb24eae27b335533220167609cfb58008c5690e140ce478acdc669f32` | `9a81e1edb24eae27b335533220167609cfb58008c5690e140ce478acdc669f32` |
| `hashes/utils.js` | `037ad49adb78168b6b699598fd33f85f7877456b1d28b4d032e2a1a16807947c` | `1ba3b1fe434318418c256236798e5afff5b4cc41773d3e3d320e4e7484beb6cb` |
| `curves/LICENSE` | `4f221aee6e072336700c408c68ab3b96a3fc09f6aebe6f48f1bd99e5ef13faec` | `4f221aee6e072336700c408c68ab3b96a3fc09f6aebe6f48f1bd99e5ef13faec` |
| `ciphers/LICENSE` | `f36671a5487c9c5050efacb58011c37c24c55a889803cb036cf9d9a6347c1e2d` | `f36671a5487c9c5050efacb58011c37c24c55a889803cb036cf9d9a6347c1e2d` |
| `hashes/LICENSE` | `4f221aee6e072336700c408c68ab3b96a3fc09f6aebe6f48f1bd99e5ef13faec` | `4f221aee6e072336700c408c68ab3b96a3fc09f6aebe6f48f1bd99e5ef13faec` |
