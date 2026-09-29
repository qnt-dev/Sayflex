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

#[test]
fn dense_list_items_become_separate_semantic_blocks() {
    let lines = vec![
        line("- First item begins here", 10, 10, 480, 990),
        line("and continues on the next rendered line.", 10, 30, 500, 980),
        line(
            "- Second item begins without a paragraph gap",
            10,
            50,
            520,
            970,
        ),
        line("and also wraps onto a second line.", 10, 70, 430, 960),
        line("• Third item uses a round bullet", 10, 90, 410, 950),
        line("with its own continuation.", 10, 110, 350, 940),
    ];

    let reconstructed = reconstruct_text(&lines, 650).unwrap();

    assert_eq!(reconstructed.blocks.len(), 3);
    assert_eq!(
        reconstructed.blocks[0].text,
        "- First item begins here\nand continues on the next rendered line."
    );
    assert_eq!(
        reconstructed.blocks[0].bounds,
        PhysicalRect {
            x: 10,
            y: 10,
            width: 500,
            height: 38,
        }
    );
    assert_eq!(
        reconstructed.blocks[1].text,
        "- Second item begins without a paragraph gap\nand also wraps onto a second line."
    );
    assert_eq!(reconstructed.blocks[1].bounds.y, 50);
    assert_eq!(
        reconstructed.blocks[2].text,
        "• Third item uses a round bullet\nwith its own continuation."
    );
}

#[test]
fn dense_column_is_bounded_without_splitting_a_wrapped_pair() {
    let lines = (0..14)
        .map(|index| {
            line(
                &format!("Dense rendered line {index}"),
                10,
                10 + index * 20,
                420,
                990,
            )
        })
        .collect::<Vec<_>>();

    let reconstructed = reconstruct_text(&lines, 650).unwrap();

    assert_eq!(
        reconstructed
            .blocks
            .iter()
            .map(|block| block.text.lines().count())
            .collect::<Vec<_>>(),
        vec![6, 6, 2]
    );
    assert_eq!(reconstructed.blocks[0].bounds.y, 10);
    assert_eq!(reconstructed.blocks[1].bounds.y, 130);
    assert_eq!(reconstructed.blocks[2].bounds.y, 250);
}

#[test]
fn lexical_hyphen_pair_never_pushes_a_dense_block_past_the_limit() {
    let lines = vec![
        line("Dense rendered line 0", 10, 10, 420, 990),
        line("Dense rendered line 1", 10, 30, 420, 990),
        line("Dense rendered line 2", 10, 50, 420, 990),
        line("Dense rendered line 3", 10, 70, 420, 990),
        line("Dense rendered line 4", 10, 90, 420, 990),
        line("trans-", 10, 110, 420, 990),
        line("lation", 10, 130, 420, 990),
        line("Dense rendered line 7", 10, 150, 420, 990),
    ];

    let reconstructed = reconstruct_text(&lines, 650).unwrap();

    assert!(
        reconstructed
            .blocks
            .iter()
            .all(|block| block.text.lines().count() <= 6)
    );
    assert!(
        reconstructed
            .blocks
            .iter()
            .any(|block| block.text.contains("translation"))
    );
    assert_eq!(reconstructed.blocks.len(), 2);
    assert_eq!(
        reconstructed.blocks[0].text,
        (0..5)
            .map(|index| format!("Dense rendered line {index}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(reconstructed.blocks[1].text.starts_with("translation"));
}

#[test]
fn ordinary_wrapped_paragraph_remains_one_block() {
    let reconstructed = reconstruct_text(
        &[
            line("A sentence wraps naturally", 10, 10, 260, 990),
            line("onto one following line.", 10, 30, 230, 990),
        ],
        650,
    )
    .unwrap();

    assert_eq!(reconstructed.blocks.len(), 1);
    assert_eq!(
        reconstructed.blocks[0].text,
        "A sentence wraps naturally\nonto one following line."
    );
}

#[test]
fn semantic_paragraph_keeps_each_ocr_line_as_a_display_element() {
    let reconstructed = reconstruct_text(
        &[
            line("A sentence wraps naturally", 10, 10, 260, 990),
            line("onto one following line.", 10, 30, 230, 980),
        ],
        650,
    )
    .unwrap();

    assert_eq!(reconstructed.blocks.len(), 1);
    assert_eq!(reconstructed.elements.len(), 2);
    assert_eq!(reconstructed.elements[0].text, "A sentence wraps naturally");
    assert_eq!(
        reconstructed.elements[0].bounds,
        PhysicalRect {
            x: 10,
            y: 10,
            width: 260,
            height: 18,
        }
    );
    assert_eq!(reconstructed.elements[1].text, "onto one following line.");
    assert_eq!(reconstructed.elements[1].bounds.y, 30);
    assert_eq!(reconstructed.elements[1].confidence_milli, 980);
}

#[test]
fn nimbus_style_card_does_not_turn_rows_and_standalone_values_into_one_mask() {
    let reconstructed = reconstruct_text(
        &[
            line("RADIUS: 450", 34, 276, 118, 990),
            line("HITS TO DESTROY: 8", 34, 300, 172, 985),
            line("CREEP HITS TO DESTROY: 16", 34, 324, 224, 980),
            line("45", 54, 366, 24, 995),
            line("275", 610, 366, 38, 995),
            line(
                "Where the thunder god's ire grows, storm clouds quickly gather.",
                34,
                408,
                520,
                970,
            ),
        ],
        650,
    )
    .unwrap();

    assert!(reconstructed.blocks.len() < reconstructed.elements.len());
    assert_eq!(reconstructed.elements.len(), 6);
    let forty_five = reconstructed
        .elements
        .iter()
        .find(|element| element.text == "45")
        .unwrap();
    let two_seventy_five = reconstructed
        .elements
        .iter()
        .find(|element| element.text == "275")
        .unwrap();
    assert_eq!(forty_five.bounds.width, 24);
    assert_eq!(two_seventy_five.bounds.x, 610);
    assert!(
        reconstructed
            .elements
            .iter()
            .all(|element| element.bounds.height == 18)
    );
}
