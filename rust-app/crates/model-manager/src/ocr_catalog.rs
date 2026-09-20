use crate::{BundleArtifact, BundleDescriptor};

const RAPID_OCR_ROOT: &str = "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2";

#[must_use]
pub fn screen_ocr_bundle() -> BundleDescriptor {
    BundleDescriptor {
        id: "screen-ocr-ppocrv5-mobile-v3.9.2".to_owned(),
        license: Some("Apache-2.0".to_owned()),
        attribution: Some(
            "PP-OCRv5 model files distributed by the RapidOCR project; model architecture by PaddleOCR"
                .to_owned(),
        ),
        artifacts: vec![
            artifact(
                "detector/ch_PP-OCRv5_det_mobile.onnx",
                "onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx",
                4_819_576,
                "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
            ),
            artifact(
                "recognizers/ch_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/ch_PP-OCRv5_rec_mobile.onnx",
                16_631_306,
                "5825fc7ebf84ae7a412be049820b4d86d77620f204a041697b0494669b1742c5",
            ),
            artifact(
                "recognizers/korean_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/korean_PP-OCRv5_rec_mobile.onnx",
                13_488_748,
                "cd6e2ea50f6943ca7271eb8c56a877a5a90720b7047fe9c41a2e541a25773c9b",
            ),
            artifact(
                "recognizers/latin_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/latin_PP-OCRv5_rec_mobile.onnx",
                7_904_513,
                "b20bd37c168a570f583afbc8cd7925603890efbcdc000a59e22c269d160b5f5a",
            ),
            artifact(
                "recognizers/eslav_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/eslav_PP-OCRv5_rec_mobile.onnx",
                7_911_802,
                "08705d6721849b1347d26187f15a5e362c431963a2a62bfff4feac578c489aab",
            ),
            artifact(
                "recognizers/en_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/en_PP-OCRv5_rec_mobile.onnx",
                7_872_351,
                "c3461add59bb4323ecba96a492ab75e06dda42467c9e3d0c18db5d1d21924be8",
            ),
            artifact(
                "recognizers/arabic_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/arabic_PP-OCRv5_rec_mobile.onnx",
                8_023_828,
                "c1192e632d0baa9146ae5b756a0e635e3dc63c1733737ebfd1629e87144e9295",
            ),
            artifact(
                "recognizers/devanagari_PP-OCRv5_rec_mobile.onnx",
                "onnx/PP-OCRv5/rec/devanagari_PP-OCRv5_rec_mobile.onnx",
                7_940_361,
                "d6f0a906580e3fa6b324a318718f1f31f268b6ea8ef985f91c2012a37f52c91e",
            ),
            artifact(
                "dictionaries/ppocrv5_dict.txt",
                "paddle/PP-OCRv5/rec/ch_PP-OCRv5_rec_mobile/ppocrv5_dict.txt",
                74_012,
                "d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b",
            ),
            artifact(
                "dictionaries/ppocrv5_en_dict.txt",
                "paddle/PP-OCRv5/rec/en_PP-OCRv5_rec_mobile/ppocrv5_en_dict.txt",
                1_416,
                "e025a66d31f327ba0c232e03f407ae8d105e1e709e7ccb3f408aa778c24e70d6",
            ),
            artifact(
                "dictionaries/ppocrv5_latin_dict.txt",
                "paddle/PP-OCRv5/rec/latin_PP-OCRv5_rec_mobile/ppocrv5_latin_dict.txt",
                1_634,
                "3c0a8a79b612653c25f765271714f71281e4e955962c153e272b7b8c1d2b13ff",
            ),
            artifact(
                "dictionaries/ppocrv5_eslav_dict.txt",
                "paddle/PP-OCRv5/rec/eslav_PP-OCRv5_rec_mobile/ppocrv5_eslav_dict.txt",
                1_663,
                "3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb",
            ),
            artifact(
                "dictionaries/ppocrv5_korean_dict.txt",
                "paddle/PP-OCRv5/rec/korean_PP-OCRv5_rec_mobile/ppocrv5_korean_dict.txt",
                47_451,
                "a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7",
            ),
            artifact(
                "dictionaries/ppocrv5_arabic_dict.txt",
                "paddle/PP-OCRv5/rec/arabic_PP-OCRv5_rec_mobile/ppocrv5_arabic_dict.txt",
                2_369,
                "7f92f7dbb9b75a4787a83bfb4f6d14a8ab515525130c9d40a9036f61cf6999e9",
            ),
            artifact(
                "dictionaries/ppocrv5_devanagari_dict.txt",
                "paddle/PP-OCRv5/rec/devanagari_PP-OCRv5_rec_mobile/ppocrv5_devanagari_dict.txt",
                1_943,
                "09c7440bfc5477e5c41052304b6b185aff8c4a5e8b2b4c23c1c706f6fe1ee9fc",
            ),
        ],
    }
}

fn artifact(relative_path: &str, remote_path: &str, bytes: u64, sha256: &str) -> BundleArtifact {
    BundleArtifact {
        relative_path: relative_path.to_owned(),
        url: format!("{RAPID_OCR_ROOT}/{remote_path}"),
        bytes,
        sha256: sha256.to_owned(),
    }
}
