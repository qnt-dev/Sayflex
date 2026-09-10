include!("../../build/windows_manifest.rs");

use std::{collections::BTreeMap, fs, path::Path};

fn main() {
    embed_windows_manifest();
    generate_typed_ui_catalog();
    #[cfg(target_os = "windows")]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../resources/icons/sayflex.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let icon = icon
            .to_str()
            .expect("SayFlex icon path must be valid Unicode");
        winresource::WindowsResource::new()
            .set_icon(icon)
            .set("ProductName", "SayFlex")
            .set("FileDescription", "SayFlex local voice and translation")
            .set("InternalName", "SayFlex")
            .set("OriginalFilename", "SayFlex.exe")
            .compile()
            .expect("failed to embed SayFlex Windows resources");
    }
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
        // Keep element identities in development and test builds so the
        // testing backend can verify the real accessibility tree. Release
        // builds do not carry this introspection metadata.
        .with_debug_info(cfg!(debug_assertions));
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");
}

fn generate_typed_ui_catalog() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_path = root.join("ui_texts.txt");
    println!("cargo:rerun-if-changed={}", source_path.display());
    let source = fs::read_to_string(&source_path).expect("failed to read ui_texts.txt");
    let entries = source
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| {
            let (name, message) = line
                .split_once('|')
                .expect("ui_texts.txt lines must be Variant|English message");
            assert!(
                name.chars()
                    .all(|character| character.is_ascii_alphanumeric()),
                "UiText variant must be an ASCII identifier: {name}"
            );
            (name.to_owned(), message.to_owned())
        })
        .collect::<Vec<_>>();

    let locales = [
        ("Russian", "ru"),
        ("Ukrainian", "uk"),
        ("Spanish", "es"),
        ("PortugueseBrazil", "pt-BR"),
        ("German", "de"),
        ("French", "fr"),
        ("Italian", "it"),
        ("Korean", "ko"),
    ];
    let catalogs = locales
        .iter()
        .map(|(_, code)| {
            let path = root
                .join("translations")
                .join(code)
                .join("LC_MESSAGES")
                .join("sayflex-desktop.po");
            println!("cargo:rerun-if-changed={}", path.display());
            let source = fs::read_to_string(path).expect("failed to read UI catalog");
            validate_po_header(&source, code);
            (*code, parse_po(&source))
        })
        .collect::<BTreeMap<_, _>>();

    let mut generated = String::from(
        "#[allow(dead_code)]\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n\
         pub(crate) enum UiText {\n",
    );
    for (name, _) in &entries {
        generated.push_str(&format!("    {name},\n"));
    }
    generated.push_str("}\n\n");
    generated.push_str("pub(crate) fn text(locale: UiLocale, message: UiText) -> &'static str {\n");
    generated.push_str("    match locale {\n");
    generated.push_str("        UiLocale::System | UiLocale::English => match message {\n");
    for (name, english) in &entries {
        generated.push_str(&format!("            UiText::{name} => {english:?},\n"));
    }
    generated.push_str("        },\n");
    for (variant, code) in locales {
        generated.push_str(&format!(
            "        UiLocale::{variant} => match message {{\n"
        ));
        let catalog = &catalogs[code];
        for (name, english) in &entries {
            let translated = catalog
                .get(english)
                .unwrap_or_else(|| panic!("{code} is missing typed UI message {english:?}"));
            let translated = canonicalize_placeholders(english, translated).unwrap_or_else(|| {
                panic!("{code} changed the number of placeholders in {english:?}")
            });
            generated.push_str(&format!("            UiText::{name} => {translated:?},\n"));
        }
        generated.push_str("        },\n");
    }
    generated.push_str("    }\n}\n");
    let output =
        Path::new(&std::env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("ui_text_catalog.rs");
    fs::write(output, generated).expect("failed to write typed UI catalog");
}

fn validate_po_header(source: &str, language: &str) {
    let expected = format!(
        "msgid \"\"\nmsgstr \"\"\n\"Language: {language}\\n\"\n\
         \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
         \"Content-Transfer-Encoding: 8bit\\n\""
    );
    assert!(
        source.replace("\r\n", "\n").starts_with(&expected),
        "{language} PO catalog must start with a contiguous gettext header"
    );
}

fn parse_po(source: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut id = None;
    for line in source.lines() {
        if let Some(value) = po_value(line, "msgid ") {
            id = (!value.is_empty()).then_some(value);
        } else if let Some(value) = po_value(line, "msgstr ")
            && let Some(id) = id.take()
            && !value.is_empty()
        {
            result.insert(id, value);
        }
    }
    result
}

fn po_value(line: &str, prefix: &str) -> Option<String> {
    let quoted = line.strip_prefix(prefix)?;
    let value = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let mut decoded = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => decoded.push('\n'),
            Some('"') => decoded.push('"'),
            Some('\\') => decoded.push('\\'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    Some(decoded)
}

fn placeholder_ranges(value: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut rest = value;
    let mut consumed = 0;
    while let Some(start) = rest.find('{') {
        let tail = &rest[start..];
        let Some(end) = tail.find('}') else {
            break;
        };
        result.push((consumed + start, consumed + start + end + 1));
        consumed += start + end + 1;
        rest = &tail[end + 1..];
    }
    result
}

fn canonicalize_placeholders(english: &str, translated: &str) -> Option<String> {
    let expected = placeholder_ranges(english)
        .into_iter()
        .map(|(start, end)| &english[start..end])
        .collect::<Vec<_>>();
    let actual = placeholder_ranges(translated);
    if expected.len() != actual.len() {
        return None;
    }
    let mut result = translated.to_owned();
    for ((start, end), replacement) in actual.into_iter().zip(expected).rev() {
        result.replace_range(start..end, replacement);
    }
    Some(result)
}
