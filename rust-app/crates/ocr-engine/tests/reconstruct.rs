use rust_ocr_engine::reconstruct_text;
use rust_protocol::{OcrFailureKind, OcrLine, PhysicalRect};

fn line(text: &str, x: i32, y: i32, width: u32, confidence_milli: u16) -> OcrLine {
    OcrLine {
        text: text.to_owned(),
        bounds: PhysicalRect {
            x,
            y,
            width,
            height: 18,
        },
        confidence_milli,
    }
}

fn two_columns_shuffled() -> Vec<OcrLine> {
    vec![
        line("Right two", 240, 42, 90, 940),
        line("Left one", 20, 10, 80, 930),
        line("Right one", 240, 10, 90, 920),
        line("Left two", 20, 42, 80, 910),
    ]
}

fn hyphenated(first: &str, second: &str) -> Vec<OcrLine> {
    vec![line(first, 10, 10, 80, 950), line(second, 10, 30, 80, 950)]
}

#[test]
fn columns_are_read_top_to_bottom_then_left_to_right() {
    assert_eq!(
        reconstruct_text(&two_columns_shuffled(), 650).unwrap().text,
        "Left one\nLeft two\n\nRight one\nRight two"
    );
}

#[test]
fn lowercase_lexical_hyphenation_is_joined() {
    assert_eq!(
        reconstruct_text(&hyphenated("trans-", "lation"), 650)
            .unwrap()
            .text,
        "translation"
    );
}

#[test]
fn identifier_like_hyphenation_is_preserved() {
    assert_eq!(
        reconstruct_text(&hyphenated("Ctrl-", "Enter"), 650)
            .unwrap()
            .text,
        "Ctrl-\nEnter"
    );
}

#[test]
fn paragraph_gap_is_preserved_inside_one_column() {
    let lines = vec![
        line("First paragraph", 10, 10, 140, 900),
        line("Second paragraph", 10, 60, 150, 900),
    ];
    assert_eq!(
        reconstruct_text(&lines, 650).unwrap().text,
        "First paragraph\n\nSecond paragraph"
    );
}

#[test]
fn reconstructed_blocks_keep_reading_order_geometry_and_confidence() {
    let reconstructed = reconstruct_text(&two_columns_shuffled(), 650).unwrap();

    assert_eq!(reconstructed.blocks.len(), 2);
    assert_eq!(reconstructed.blocks[0].text, "Left one\nLeft two");
    assert_eq!(reconstructed.blocks[0].bounds.x, 20);
    assert_eq!(reconstructed.blocks[0].bounds.y, 10);
    assert_eq!(reconstructed.blocks[0].bounds.width, 80);
    assert_eq!(reconstructed.blocks[0].bounds.height, 50);
    assert_eq!(reconstructed.blocks[0].confidence_milli, 920);
    assert_eq!(reconstructed.blocks[1].text, "Right one\nRight two");
}

#[test]
fn low_confidence_noise_is_not_translated() {
    assert_eq!(
        reconstruct_text(&[line("noise", 0, 0, 20, 200)], 650),
        Err(OcrFailureKind::NoText)
    );
}

#[test]
fn whitespace_only_regions_are_not_text() {
    assert_eq!(
        reconstruct_text(&[line("  \n ", 0, 0, 20, 990)], 650),
        Err(OcrFailureKind::NoText)
    );
}
