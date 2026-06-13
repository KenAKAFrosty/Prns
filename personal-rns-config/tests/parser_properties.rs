use personal_rns_config::configobj::{self, Section, Value};
use personal_rns_config::reference;
use proptest::prelude::*;

fn arb_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        "[a-z0-9_.:]{1,10}".prop_map(Value::Scalar),
        prop::collection::vec("[a-z0-9_.:]{1,8}", 1..4).prop_map(Value::List),
    ]
}

fn dedup_keyed<T>(pairs: Vec<(String, T)>) -> Vec<(String, T)> {
    let mut seen = std::collections::HashSet::new();
    pairs
        .into_iter()
        .filter(|(key, _)| seen.insert(key.clone()))
        .collect()
}

fn arb_scalars() -> impl Strategy<Value = Vec<(String, Value)>> {
    prop::collection::vec(("[a-z][a-z0-9_]{0,7}", arb_value()), 0..5).prop_map(dedup_keyed)
}

fn arb_section() -> impl Strategy<Value = Section> {
    let leaf = arb_scalars().prop_map(|scalars| Section {
        scalars,
        sections: Vec::new(),
    });
    leaf.prop_recursive(3, 24, 3, |inner| {
        (
            arb_scalars(),
            prop::collection::vec(("[a-z][a-z0-9_]{0,7}", inner), 0..3),
        )
            .prop_map(|(scalars, sections)| Section {
                scalars,
                sections: dedup_keyed(sections),
            })
    })
}

fn emit_value(value: &Value, out: &mut String) {
    match value {
        Value::Scalar(text) => out.push_str(text),
        Value::List(items) => {
            out.push_str(&items.join(", "));
            if items.len() == 1 {
                out.push(',');
            }
        }
    }
}

fn emit_section(section: &Section, depth: usize, out: &mut String) {
    for (key, value) in &section.scalars {
        out.push_str(&format!("{key} = "));
        emit_value(value, out);
        out.push('\n');
    }
    for (name, sub) in &section.sections {
        let brackets = depth + 1;
        out.push_str(&"[".repeat(brackets));
        out.push_str(name);
        out.push_str(&"]".repeat(brackets));
        out.push('\n');
        emit_section(sub, depth + 1, out);
    }
}

proptest! {
    #[test]
    fn configobj_parse_never_panics_on_arbitrary_text(text in ".*") {
        let _ = configobj::parse(&text);
    }

    #[test]
    fn reference_parse_never_panics_on_arbitrary_text(text in ".*") {
        let _ = reference::parse(&text);
    }

    #[test]
    fn neither_layer_panics_on_structural_noise(
        text in "[\\[\\]=#'\"a-z0-9 \t\n]{0,300}"
    ) {
        let _ = configobj::parse(&text);
        let _ = reference::parse(&text);
    }

    #[test]
    fn a_section_tree_survives_emit_then_parse(section in arb_section()) {
        let mut text = String::new();
        emit_section(&section, 0, &mut text);
        let reparsed = configobj::parse(&text).expect("emitted text parses");
        prop_assert_eq!(reparsed, section);
    }
}
