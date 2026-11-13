use std::collections::{BTreeMap, BTreeSet};

use mcp_policy::ByteBudget;

use crate::syntax::is_variable;

enum Piece<'a> {
    Text(&'a str),
    Variable(&'a str),
}

fn parse(mut text: &str) -> Option<Vec<Piece<'_>>> {
    let mut pieces = Vec::new();
    while let Some(index) = text.find(['{', '}']) {
        let (literal, rest) = text.split_at(index);
        pieces.push(Piece::Text(literal));
        if rest.starts_with("{{") || rest.starts_with("}}") {
            pieces.push(Piece::Text(&rest[..1]));
            text = &rest[2..];
            continue;
        }
        let (name, remainder) = rest.strip_prefix('{')?.split_once('}')?;
        if !is_variable(name) {
            return None;
        }
        pieces.push(Piece::Variable(name));
        text = remainder;
    }
    pieces.push(Piece::Text(text));
    Some(pieces)
}

pub fn prompt_variables(text: &str) -> Option<BTreeSet<&str>> {
    Some(
        parse(text)?
            .into_iter()
            .filter_map(|piece| match piece {
                Piece::Variable(name) => Some(name),
                Piece::Text(_) => None,
            })
            .collect(),
    )
}

pub(crate) fn render(
    text: &str,
    arguments: &BTreeMap<String, String>,
    budget: &mut ByteBudget,
) -> Option<String> {
    let mut output = String::new();
    for piece in parse(text)? {
        let text = match piece {
            Piece::Text(text) => text,
            Piece::Variable(name) => arguments.get(name).map_or("", String::as_str),
        };
        budget.consume(text.len()).ok()?;
        output.push_str(text);
    }
    Some(output)
}
