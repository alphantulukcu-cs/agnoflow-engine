#[derive(Debug)]
pub struct Pipeline {
    pub steps: Vec<Step>,
}

/// `[sube]`          → TypeFilter { key: "type", val: "sube" }
/// `[special:doviz]` → TypeFilter { key: "special", val: "doviz" }
#[derive(Debug, PartialEq)]
pub struct TypeFilter {
    pub key: String,
    pub val: String,
}

impl TypeFilter {
    fn new(key: impl Into<String>, val: impl Into<String>) -> Self {
        Self { key: key.into(), val: val.into() }
    }
}

/// Boolean filter expression inside `[...]`.
#[derive(Debug, PartialEq)]
pub enum FilterExpr {
    Leaf(TypeFilter),
    Not(Box<FilterExpr>),
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Parent,
    Siblings,
    SiblingsT(FilterExpr),
    Children,
    ChildrenT(FilterExpr),
    UpT(FilterExpr),
    DownT(FilterExpr),
    Ancestors,
    AncestorsT(FilterExpr),
    /// `*:[filter]` — anchor'dan BAĞIMSIZ, tenant genelinde tipe göre KAYNAK küme.
    /// Pipeline'ın ilk adımı olarak gelir; ardından normal adımlarla zincirlenebilir
    /// (ör. `*:[type:sube].parent` = tüm şube'lerin parentları).
    GlobalType(FilterExpr),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("expression must start with 'self'")]
    MissingSelf,
    #[error("unknown step: {0:?}")]
    UnknownStep(String),
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
}

pub fn parse(expr: &str) -> Result<Pipeline, ParseError> {
    let expr = expr.trim();

    // Global tip selektörü: "*:[filter]" — "self" köküne bağlı DEĞİL, tenant genelinde bir
    // KAYNAK küme. İlk adım olur; ardından normal adımlarla zincirlenebilir
    // (ör. "*:[type:sube].parent" = tüm şube'lerin parentları).
    if let Some(after) = expr.strip_prefix("*:") {
        if !after.starts_with('[') {
            return Err(ParseError::InvalidFilter(
                format!("global tip selektörü '[filter]' bekliyor: {expr}"),
            ));
        }
        let close = after
            .find(']')
            .ok_or_else(|| ParseError::InvalidFilter(format!("kapanmayan '[': {expr}")))?;
        let filter = parse_filter_expr(&after[1..close])?;
        let mut steps = vec![Step::GlobalType(filter)];
        let remainder = &after[close + 1..];
        if !remainder.is_empty() {
            let rest = remainder
                .strip_prefix('.')
                .ok_or_else(|| ParseError::UnknownStep(remainder.to_string()))?;
            for tok in split_tokens(rest) {
                steps.push(parse_step(tok)?);
            }
        }
        return Ok(Pipeline { steps });
    }

    let rest = expr
        .strip_prefix("self")
        .ok_or(ParseError::MissingSelf)?;

    if rest.is_empty() {
        return Ok(Pipeline { steps: vec![] });
    }

    if !rest.starts_with('.') {
        return Err(ParseError::MissingSelf);
    }

    let tokens = split_tokens(&rest[1..]);
    let steps = tokens
        .into_iter()
        .map(parse_step)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Pipeline { steps })
}

fn split_tokens(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => {
                tokens.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    tokens.push(&s[start..]);
    tokens
}

fn parse_type_filter(inner: &str) -> TypeFilter {
    match inner.split_once(':') {
        Some((key, val)) => TypeFilter::new(key, val),
        None             => TypeFilter::new("type", inner),
    }
}

#[derive(Debug)]
enum FTok {
    Ident(String),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn tokenize_filter(s: &str) -> Result<Vec<FTok>, ParseError> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => { chars.next(); }
            '!' => { chars.next(); out.push(FTok::Not); }
            '(' => { chars.next(); out.push(FTok::LParen); }
            ')' => { chars.next(); out.push(FTok::RParen); }
            '&' => {
                chars.next();
                if chars.next() == Some('&') {
                    out.push(FTok::And);
                } else {
                    return Err(ParseError::InvalidFilter("expected '&&'".to_string()));
                }
            }
            '|' => {
                chars.next();
                if chars.next() == Some('|') {
                    out.push(FTok::Or);
                } else {
                    return Err(ParseError::InvalidFilter("expected '||'".to_string()));
                }
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' => {
                let mut ident = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '-' || c == ':' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(FTok::Ident(ident));
            }
            c => return Err(ParseError::InvalidFilter(format!("unexpected '{c}'"))),
        }
    }
    Ok(out)
}

struct FParser {
    tokens: Vec<FTok>,
    pos:    usize,
}

impl FParser {
    fn new(tokens: Vec<FTok>) -> Self { Self { tokens, pos: 0 } }

    fn peek(&self) -> Option<&FTok> { self.tokens.get(self.pos) }

    fn advance(&mut self) { self.pos += 1; }

    fn parse(&mut self) -> Result<FilterExpr, ParseError> {
        let expr = self.parse_or()?;
        if self.peek().is_some() {
            return Err(ParseError::InvalidFilter("unexpected tokens after expression".to_string()));
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<FilterExpr, ParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(FTok::Or)) {
            self.advance();
            let right = self.parse_and()?;
            left = match left {
                FilterExpr::Or(mut v) => { v.push(right); FilterExpr::Or(v) }
                other                 => FilterExpr::Or(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<FilterExpr, ParseError> {
        let mut left = self.parse_not()?;
        while matches!(self.peek(), Some(FTok::And)) {
            self.advance();
            let right = self.parse_not()?;
            left = match left {
                FilterExpr::And(mut v) => { v.push(right); FilterExpr::And(v) }
                other                  => FilterExpr::And(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<FilterExpr, ParseError> {
        if matches!(self.peek(), Some(FTok::Not)) {
            self.advance();
            let inner = self.parse_not()?;
            Ok(FilterExpr::Not(Box::new(inner)))
        } else {
            self.parse_atom()
        }
    }

    fn parse_atom(&mut self) -> Result<FilterExpr, ParseError> {
        match self.peek() {
            Some(FTok::LParen) => {
                self.advance();
                let expr = self.parse_or()?;
                if !matches!(self.peek(), Some(FTok::RParen)) {
                    return Err(ParseError::InvalidFilter("expected ')'".to_string()));
                }
                self.advance();
                Ok(expr)
            }
            Some(FTok::Ident(_)) => {
                let s = match &self.tokens[self.pos] {
                    FTok::Ident(s) => s.clone(),
                    _ => unreachable!(),
                };
                self.advance();
                Ok(FilterExpr::Leaf(parse_type_filter(&s)))
            }
            None  => Err(ParseError::InvalidFilter("expected filter term".to_string())),
            other => Err(ParseError::InvalidFilter(format!("unexpected token {:?}", other))),
        }
    }
}

fn parse_filter_expr(inner: &str) -> Result<FilterExpr, ParseError> {
    let tokens = tokenize_filter(inner)?;
    if tokens.is_empty() {
        return Err(ParseError::InvalidFilter("empty filter".to_string()));
    }
    FParser::new(tokens).parse()
}

fn parse_step(token: &str) -> Result<Step, ParseError> {
    match token {
        "parent"    => return Ok(Step::Parent),
        "siblings"  => return Ok(Step::Siblings),
        "children"  => return Ok(Step::Children),
        "ancestors" => return Ok(Step::Ancestors),
        _           => {}
    }

    if let Some(rest) = token.strip_prefix("siblings[") {
        let inner = rest.strip_suffix(']').ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        if inner.is_empty() { return Err(ParseError::UnknownStep(token.to_string())); }
        return Ok(Step::SiblingsT(parse_filter_expr(inner)?));
    }

    if let Some(rest) = token.strip_prefix("children[") {
        let inner = rest.strip_suffix(']').ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        if inner.is_empty() { return Err(ParseError::UnknownStep(token.to_string())); }
        return Ok(Step::ChildrenT(parse_filter_expr(inner)?));
    }

    if let Some(rest) = token.strip_prefix("up[") {
        let inner = rest.strip_suffix(']').ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        if inner.is_empty() { return Err(ParseError::UnknownStep(token.to_string())); }
        return Ok(Step::UpT(parse_filter_expr(inner)?));
    }

    if let Some(rest) = token.strip_prefix("down[") {
        let inner = rest.strip_suffix(']').ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        if inner.is_empty() { return Err(ParseError::UnknownStep(token.to_string())); }
        return Ok(Step::DownT(parse_filter_expr(inner)?));
    }

    if let Some(rest) = token.strip_prefix("ancestors[") {
        let inner = rest.strip_suffix(']').ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        if inner.is_empty() { return Err(ParseError::UnknownStep(token.to_string())); }
        return Ok(Step::AncestorsT(parse_filter_expr(inner)?));
    }

    Err(ParseError::UnknownStep(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(expr: &str) -> Vec<Step> {
        parse(expr).unwrap().steps
    }

    fn tf(key: &str, val: &str) -> TypeFilter { TypeFilter::new(key, val) }

    fn leaf(val: &str) -> FilterExpr { FilterExpr::Leaf(TypeFilter::new("type", val)) }
    fn kleaf(key: &str, val: &str) -> FilterExpr { FilterExpr::Leaf(tf(key, val)) }

    #[test]
    fn test_bare_self() { assert_eq!(steps("self"), vec![]); }

    #[test]
    fn test_parent() { assert_eq!(steps("self.parent"), vec![Step::Parent]); }

    #[test]
    fn test_siblings() { assert_eq!(steps("self.siblings"), vec![Step::Siblings]); }

    #[test]
    fn test_siblings_t() {
        assert_eq!(steps("self.siblings[sube]"), vec![Step::SiblingsT(leaf("sube"))]);
    }

    #[test]
    fn test_children() { assert_eq!(steps("self.children"), vec![Step::Children]); }

    #[test]
    fn test_children_t() {
        assert_eq!(steps("self.children[il]"), vec![Step::ChildrenT(leaf("il"))]);
    }

    #[test]
    fn test_up_t() { assert_eq!(steps("self.up[bolge]"), vec![Step::UpT(leaf("bolge"))]); }

    #[test]
    fn test_two_step_chain() {
        assert_eq!(
            steps("self.up[bolge].children[il]"),
            vec![Step::UpT(leaf("bolge")), Step::ChildrenT(leaf("il"))]
        );
    }

    #[test]
    fn test_three_step_chain() {
        assert_eq!(
            steps("self.up[bolge].children[il].children[sube]"),
            vec![Step::UpT(leaf("bolge")), Step::ChildrenT(leaf("il")), Step::ChildrenT(leaf("sube"))]
        );
    }

    #[test]
    fn test_siblings_then_children() {
        assert_eq!(
            steps("self.siblings.children[kredi]"),
            vec![Step::Siblings, Step::ChildrenT(leaf("kredi"))]
        );
    }

    #[test]
    fn test_missing_self() {
        assert!(matches!(parse("children"), Err(ParseError::MissingSelf)));
    }

    #[test]
    fn test_unknown_step() {
        assert!(matches!(parse("self.garbage"), Err(ParseError::UnknownStep(_))));
    }

    #[test]
    fn test_whitespace_trimmed() { assert_eq!(steps("  self  "), vec![]); }

    #[test]
    fn test_key_val_children() {
        assert_eq!(steps("self.children[special:doviz]"), vec![Step::ChildrenT(kleaf("special", "doviz"))]);
    }

    #[test]
    fn test_empty_type_rejected() {
        assert!(matches!(parse("self.siblings[]"), Err(ParseError::UnknownStep(_))));
        assert!(matches!(parse("self.children[]"), Err(ParseError::UnknownStep(_))));
        assert!(matches!(parse("self.up[]"),       Err(ParseError::UnknownStep(_))));
    }

    #[test]
    fn test_and_filter() {
        assert_eq!(
            steps("self.children[special:kredi && type:sube]"),
            vec![Step::ChildrenT(FilterExpr::And(vec![kleaf("special", "kredi"), leaf("sube")]))]
        );
    }

    #[test]
    fn test_or_filter() {
        assert_eq!(
            steps("self.siblings[type:sube || type:ilce]"),
            vec![Step::SiblingsT(FilterExpr::Or(vec![leaf("sube"), leaf("ilce")]))]
        );
    }

    #[test]
    fn test_not_filter() {
        assert_eq!(
            steps("self.children[!type:root]"),
            vec![Step::ChildrenT(FilterExpr::Not(Box::new(leaf("root"))))]
        );
    }

    #[test]
    fn test_down_t() {
        assert_eq!(steps("self.down[bolge]"), vec![Step::DownT(leaf("bolge"))]);
    }

    #[test]
    fn test_ancestors() {
        assert_eq!(steps("self.ancestors"), vec![Step::Ancestors]);
    }

    #[test]
    fn test_ancestors_t() {
        assert_eq!(steps("self.ancestors[bolge]"), vec![Step::AncestorsT(leaf("bolge"))]);
    }

    #[test]
    fn test_user_example() {
        assert_eq!(
            steps("self.parent.siblings.children[special:kredi && type:sube]"),
            vec![
                Step::Parent,
                Step::Siblings,
                Step::ChildrenT(FilterExpr::And(vec![kleaf("special", "kredi"), leaf("sube")])),
            ]
        );
    }

    #[test]
    fn test_global_type() {
        // *:[type:sube] — tenant genelinde tip selektörü, tek adım.
        assert_eq!(steps("*:[type:sube]"), vec![Step::GlobalType(leaf("sube"))]);
        assert_eq!(steps("*:[sube]"), vec![Step::GlobalType(leaf("sube"))]);
        assert_eq!(
            steps("*:[type:sube || type:ilce]"),
            vec![Step::GlobalType(FilterExpr::Or(vec![leaf("sube"), leaf("ilce")]))]
        );
    }

    #[test]
    fn test_global_type_missing_bracket_rejected() {
        assert!(matches!(parse("*:type:sube"), Err(ParseError::InvalidFilter(_))));
    }

    #[test]
    fn test_global_type_chained() {
        // *:[type:sube].parent = tüm şube'lerin parentları (global kaynak + zincir)
        assert_eq!(
            steps("*:[type:sube].parent"),
            vec![Step::GlobalType(leaf("sube")), Step::Parent]
        );
        assert_eq!(
            steps("*:[type:sube].children[il]"),
            vec![Step::GlobalType(leaf("sube")), Step::ChildrenT(leaf("il"))]
        );
    }
}
