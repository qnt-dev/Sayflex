# Screen translation OCR benchmark

- Generated (UTC): 2026-09-28T15:02:43Z
- Decision: **Shipped PP-OCR worker path verified**
- `screen_translation_packaging=true`
- Iterations per fixture and candidate: 30
- Corpus: 17 immutable PNG fixtures covering all 14 SayFlex source languages plus small mixed text, two columns, and blank input
- CPU: Intel(R) Core(TM) Ultra 7 270K Plus
- Windows: Microsoft Windows 11 Pro build 26200
- Rust: rustc 1.95.0 (59807616e 2026-04-14)
- OCR runtime: shipped `SayFlex.Ocr.exe` JSON worker protocol, `oar-ocr 0.9.2` + `ort 2.0.0-rc.13` on CPU

## Admission rule

The release gate is the shipped `SayFlex.Ocr.exe` plus its verified bundle, driven through the same JSON protocol as the desktop application. It must complete every fixture after a real worker restart and keep normal light/dark crop p95 below 500 ms. Direct PP-OCR, Tesseract, and Windows Media OCR are diagnostic comparators; a missing comparator does not negate a successful shipped-path measurement.

| Engine | All rows available | Aggregate CER | Exact rate | p50 ms | p95 ms | Normal p95 ms | Max working set bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| sayflex-ocr-worker-cpu | True | 0.046 | 0.824 | 29.000 | 90.000 | 32.000 | 387948544.000 |
| ppocr-v5-onnx-cpu | True | 0.032 | 0.882 | 28.630 | 46.984 | 31.107 | n/a |
| tesseract-5 | False | n/a | n/a | n/a | n/a | n/a | n/a |
| windows-media-ocr | False | n/a | n/a | n/a | n/a | n/a | n/a |

Shipped worker completed every row: **True**

OSS-compatible runtime and model license: **True**

Worker restart generation at measurement: **2** (expected at least 2): **True**

Normal-crop p95 below 500 ms: **True**

Peak worker working set observed: **399478784.000 bytes**

Direct PP-OCR lower aggregate CER than Tesseract when both comparators were available: **False**

## Shipped worker CER by category

| Category | Mean CER | Rows |
|---|---:|---:|
| blank | 0.000 | 30 |
| columns | 0.395 | 30 |
| dark | 0.000 | 210 |
| light | 0.021 | 210 |
| small | 0.240 | 30 |

## Failures and unavailable candidates

- **sayflex-ocr-worker-cpu:** none
- **ppocr-v5-onnx-cpu:** none
- **tesseract-5:** --tesseract-exe was not supplied
- **windows-media-ocr:** benchmark process has no Windows package identity

## Observed quality limitations

- `light-hi.png` (CER 0.147): expected 'चुने हुए स्क्रीन क्षेत्र का अनुवाद'; recognized 'चुनेहुए स्क्ी क्े्र का अनुवाद'.
- `small-mixed.png` (CER 0.240): expected 'Status: готово — build 42'; recognized 'Status: rotoBo — build 42'.
- `two-columns.png` (CER 0.395): expected 'Left one / Left two /  / Right one / Right two'; recognized 'Left one / Right one / Left two / Right two'.

The two-column divergence is intentionally retained for the geometry-based reading-order stage; raw detector order is not treated as final paragraph order. The Hindi divergence is a recognizer-quality limitation and is not hidden by normalization.

## Verified RapidOCR v3.9.2 artifacts

Source metadata: https://github.com/RapidAI/RapidOCR/blob/main/python/rapidocr/default_models.yaml  
License: Apache-2.0 (PaddleOCR / RapidOCR). Published ONNX hashes are retained verbatim. Dictionary bytes are pinned after TLS retrieval from the versioned RapidOCR URLs because the upstream manifest publishes dictionary URLs but not dictionary hashes. Models remain benchmark cache files under `rust-app/target` and are not committed.

- `ch_PP-OCRv5_det_mobile.onnx`: `4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae`
- `ch_PP-OCRv5_rec_mobile.onnx`: `5825fc7ebf84ae7a412be049820b4d86d77620f204a041697b0494669b1742c5`
- `korean_PP-OCRv5_rec_mobile.onnx`: `cd6e2ea50f6943ca7271eb8c56a877a5a90720b7047fe9c41a2e541a25773c9b`
- `latin_PP-OCRv5_rec_mobile.onnx`: `b20bd37c168a570f583afbc8cd7925603890efbcdc000a59e22c269d160b5f5a`
- `eslav_PP-OCRv5_rec_mobile.onnx`: `08705d6721849b1347d26187f15a5e362c431963a2a62bfff4feac578c489aab`
- `en_PP-OCRv5_rec_mobile.onnx`: `c3461add59bb4323ecba96a492ab75e06dda42467c9e3d0c18db5d1d21924be8`
- `arabic_PP-OCRv5_rec_mobile.onnx`: `c1192e632d0baa9146ae5b756a0e635e3dc63c1733737ebfd1629e87144e9295`
- `devanagari_PP-OCRv5_rec_mobile.onnx`: `d6f0a906580e3fa6b324a318718f1f31f268b6ea8ef985f91c2012a37f52c91e`
- `ppocrv5_dict.txt`: `d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b`
- `ppocrv5_en_dict.txt`: `e025a66d31f327ba0c232e03f407ae8d105e1e709e7ccb3f408aa778c24e70d6`
- `ppocrv5_latin_dict.txt`: `3c0a8a79b612653c25f765271714f71281e4e955962c153e272b7b8c1d2b13ff`
- `ppocrv5_eslav_dict.txt`: `3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb`
- `ppocrv5_korean_dict.txt`: `a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7`
- `ppocrv5_arabic_dict.txt`: `7f92f7dbb9b75a4787a83bfb4f6d14a8ab515525130c9d40a9036f61cf6999e9`
- `ppocrv5_devanagari_dict.txt`: `09c7440bfc5477e5c41052304b6b185aff8c4a5e8b2b4c23c1c706f6fe1ee9fc`

## Runtime artifact hashes

The shipped worker contains the ONNX Runtime CPU path used by SayFlex. `DirectML.dll` may be copied by the upstream `ort` build bundle, but no DirectML execution provider is enabled or selected by the OCR worker.

- `SayFlex.Ocr.exe`: `8686e968e482a89ad6a7bbd28e3fb594dbd7fef8a6e42824baf9635594ff5e86`
- `sayflex-ocr-benchmark.exe`: `2aa9e6675665ef8607744496dbfceb32431c3cbcd8c8e6516525ff0b45294498`
- `DirectML.dll`: `9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1`

## Immutable fixture hashes

- `blank.png`: `224d8fcb02239de7db0fef141e274a163a2c86034686cd70716615b1c36d83e7`
- `dark-ar.png`: `80c33e08ec8d92cbebbfd65c33ae5391d2e2742ee57ce6ec5c2f0efbb3ae1c51`
- `dark-de.png`: `82b328f2bd6daf9b4a141c024ddeb56eb4c437cccecf9e1485e7740427d1805f`
- `dark-es.png`: `335bc8fc0542181910450631436c577aaa89a731bf07020bc2dcd6370aed564d`
- `dark-it.png`: `fe925361eed52049893810d4ed3c86d10d390a7ea6c36c78224fbcea44f3b77e`
- `dark-ko.png`: `32c77a1c5ec6de6b99764acb26deb3f3a6b6de075e7b0e11d6d9458fa02191ee`
- `dark-ru.png`: `b4a63657522a41a9040f15c70be67c24d0e6289c8b8aeb7a0783c4c6db087e92`
- `dark-zh-hant.png`: `6cddcbd1435ceb8e019f8cbb520533df75928c429cfdc139709c64a21bdb91f5`
- `light-en.png`: `c024d93fb4ff4ae450b4cefcfb2c1671628084bdda1e191b7aab77c771090931`
- `light-fr.png`: `e0ca3e347bccf52f8c80b4c9680db381eaa2115a8cb1571c8090de01b6c3f897`
- `light-hi.png`: `c37947cfb3ef23fd702e02a87ee36451ea565f99848694804f1ddffa24d6730c`
- `light-ja.png`: `2c5e4c7576d8996f781d6615b63a906e6fa6b09ec4dbda82ec8d84ce9df73872`
- `light-pt-br.png`: `03a5d877c4126d9c170beefb47aa8855367081bb7834a3b06ba244f0fe39d57d`
- `light-uk.png`: `88ee736cd03272ea066e1826e2bf6a33e5c14e05ea2da24916635ef0b4e095d9`
- `light-zh-hans.png`: `33bf87095f3c6a318ccab632e6ff02ccec61910b8550bab47750ee37aadcb65a`
- `small-mixed.png`: `c08d26238131341b1c34f5cb8492114727e8b40d668305ffd3ddb2c9ef01406a`
- `two-columns.png`: `156469b858a86e69c89df5fbc54ce8e0797e34735ead6f5fbb968d163ccd2ccf`

## Raw data

Machine-readable JSON Lines are generated at `rust-app/target/ocr-benchmark-results.jsonl`. Each shipped-worker row includes protocol latency, working-set observations, and the worker generation proving the post-warmup restart path. Expected and recognized strings are fixture-only data; no user capture is read or retained by this benchmark.