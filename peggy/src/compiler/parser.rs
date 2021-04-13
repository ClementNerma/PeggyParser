use super::errors::{add_base_err_loc, ParserError, ParserErrorContent};
use super::singles;
use super::utils::*;
use super::validator::validate_parsed_peg;
use std::collections::HashMap;
use std::rc::Rc;

/// A grammar's entrypoint rule
pub const GRAMMAR_ENTRYPOINT_RULE: &str = "main";

/// Compile a Peggy grammar to a [syntax tree](`PegSyntaxTree`)
pub fn parse_peg(grammar: &str) -> Result<PegSyntaxTree, ParserError> {
    let parsed = parse_peg_nocheck(grammar)?;

    // Ensure the syntax tree is valid
    validate_parsed_peg(&parsed)?;

    Ok(parsed)
}

/// Compile a Peggy grammar but don't check for validity (e.g. inexistant rule names, etc.)
///
/// A bit faster than [`parse_peg`] but less safe due to the lack of check.
///
/// Near-instant checks are still performed, like ensuring the presence of a `main` rule.
pub fn parse_peg_nocheck(input: &str) -> Result<PegSyntaxTree, ParserError> {
    // Collected rules
    let mut rules = HashMap::new();

    // Is a multi-line comment opened?
    let mut multi_line_comment_opened = None;

    // Iterate over each line, as there should be one rule per non-empty line
    for (l, line) in input.lines().enumerate() {
        // Left trim
        let (line, trimmed) = trim_start_and_count(line);

        if line.trim_end() == "###" {
            multi_line_comment_opened = if multi_line_comment_opened.is_none() {
                Some((l, trimmed))
            } else {
                None
            };
            continue;
        }

        if multi_line_comment_opened.is_some() {
            continue;
        }

        // Ignore empty lines
        if is_finished_line(line) {
            continue;
        }

        let mut chars = line.chars();

        // Get the first character of the line...
        let c = chars.next().unwrap();

        // ...which must be a rule's name (syntax: `rule = <content>`)
        if !c.is_alphabetic() && c != '_' {
            return Err(ParserError::new(
                ParserLoc::new(l, trimmed),
                1,
                ParserErrorContent::ExpectedRuleDeclaration,
                Some(match c {
                    '0'..='9' => "digits are not allowed to begin a rule's name",
                    _ => "only alphabetic and underscores characters are allowed to begin a rule's name"
                })
            ));
        }

        // Length of the rule's name (1 as we already checked the first character)
        let mut rule_name_length = 1;

        // Indicate if the name is finished, but the loop is still active.
        // Used to count spaces separating the name and the assignment operator (=)
        let mut rule_name_ended = false;

        // Number of spaces separating the name and the assignment operator (=)
        let mut rule_op_spaces_sep = 0;

        // Iterate over characters
        loop {
            match chars.next() {
                // Identifier-compliant characters
                Some(c) if !rule_name_ended && (c.is_alphanumeric() || c == '_') => {
                    rule_name_length += 1;
                }

                // Assignment operator (indicates the beginning of the rule's content)
                Some('=') => break,

                // Whitespaces (optional, can be used to separate the name and the assignment operator)
                Some(c) if c.is_whitespace() => {
                    rule_name_ended = true;
                    rule_op_spaces_sep += 1;
                }

                // Other characters (= non-compliant)
                Some(c) => {
                    return Err(ParserError::new(
                        ParserLoc::new(l, trimmed + rule_name_length),
                        1,
                        ParserErrorContent::IllegalSymbol(c),
                        Some("only alphanumeric and underscore characters are allowed in a rule's name")
                    ));
                }

                // End of line without an assignment operator
                None => {
                    return Err(ParserError::new(
                        ParserLoc::new(l, trimmed + rule_name_length),
                        1,
                        ParserErrorContent::ExpectedRuleAssignmentOp,
                        Some("you may have forgot to add the rule assignment operator '='"),
                    ));
                }
            }
        }

        // Collect the rule's name
        let rule_name = &line[..rule_name_length];

        // Detect reserved rule names
        if is_reserved_rule_name(rule_name) {
            return Err(ParserError::new(
                ParserLoc::new(l, trimmed),
                rule_name_length,
                ParserErrorContent::ReservedUppercaseRuleName,
                Some("try to use a name that doesn't start by 'B_' (builtin rules) or 'E_' (external rules)"),
            ));
        }

        // Detect duplicate rules
        if rules.contains_key(rule_name) {
            return Err(ParserError::new(
                ParserLoc::new(l, trimmed),
                rule_name_length,
                ParserErrorContent::DuplicateRuleName,
                None,
            ));
        }

        // Get the column the rule's content starts at
        let mut start_column = rule_name_length + rule_op_spaces_sep + 1;

        // Left-trim the content
        let (line, trimmed_2) = trim_start_and_count(&line[start_column..]);
        start_column += trimmed + trimmed_2;

        // Parse the rule's content
        let rule = parse_rule_content(
            line,
            ParserLoc {
                line: l,
                col: start_column,
            },
        )?;

        // Save the new rule
        rules.insert(rule_name, rule);
    }

    // Ensure all multi-line comments have been closed
    if let Some((line, col)) = multi_line_comment_opened {
        return Err(ParserError::new(
            ParserLoc::new(input.lines().count(), 0),
            0,
            ParserErrorContent::UnterminatedMultiLineComment {
                started_at: ParserLoc::new(line, col),
            },
            Some("you can add '###' on a single line to close the comment"),
        ));
    }

    // Ensure a main rule is declared
    if !rules.contains_key(GRAMMAR_ENTRYPOINT_RULE) {
        return Err(ParserError::new(
            ParserLoc::new(input.lines().count(), 0),
            0,
            ParserErrorContent::MissingMainRule,
            Some(
                "you must declare a rule named 'main' which will be the entrypoint of your syntax",
            ),
        ));
    }

    // Success!
    Ok(PegSyntaxTree { rules })
}

/// Parse a rule's content (e.g. `<content>` in `rule = <content>`)
pub fn parse_rule_content(input: &str, base_loc: ParserLoc) -> Result<RuleContent, ParserError> {
    // This function is not supposed to be called with an empty content, so we can directly parse the first pattern of the rule
    let (first_pattern, pattern_len, stopped_at) =
        parse_pattern(input).map_err(|err| add_base_err_loc(base_loc.line, base_loc.col, err))?;

    // Remove the first pattern's content from the remaining input
    let input = &input[pattern_len..];
    let mut column = pattern_len;

    // Make a global pattern
    // This will contain everything the rule's content is made of
    // The data is built depending on the reason why the pattern parser stopped previously
    let mut global_pattern = match stopped_at {
        // If the parser stopped because it got to the end of the line, return the pattern as it is
        PatternParserStoppedAt::End => first_pattern,

        // If the parser stopped because of a continuation separator (whitespace) or an union separator (|),
        // all items of the follow/union should be collected at once
        PatternParserStoppedAt::ContinuationSep | PatternParserStoppedAt::UnionSep => {
            // Create an union child (see below)
            fn create_union_child(mut patterns: Vec<Pattern>) -> Pattern {
                assert_ne!(patterns.len(), 0);

                if patterns.len() == 1 {
                    // Avoid making a `RulePatternValue::Suite` wrapper for a single pattern
                    patterns.into_iter().next().unwrap()
                } else {
                    let ParserLoc { line, col } = patterns[0].relative_loc;

                    for pattern in patterns.iter_mut() {
                        pattern.relative_loc = sub_parser_loc(line, col, pattern.relative_loc);
                    }

                    Pattern {
                        relative_loc: patterns[0].relative_loc,
                        repetition: None,
                        is_silent: false,
                        value: RulePatternValue::Suite(patterns),
                    }
                }
            }

            // Get the first patterns
            // The `patterns` variable contains the parsed patterns
            // The `unions` variable contains each member of the pending union. If the whole rule's content is not an union, this will remain empty.
            // When an union separator is detected, the content of `patterns` is moved to `unions` in order to separate each member of the union.
            let (mut patterns, mut unions) = if stopped_at == PatternParserStoppedAt::UnionSep {
                (vec![], vec![create_union_child(vec![first_pattern])])
            } else {
                (vec![first_pattern], vec![])
            };

            // Local modifyable input
            let mut input = input;

            loop {
                // Parse the next pattern
                let (next_pattern, next_pattern_len, next_stopped_at) = parse_pattern(input)
                    .map_err(|err| add_base_err_loc(base_loc.line, base_loc.col + column, err))?;

                // Push it to the list of pending patterns
                patterns.push(Pattern {
                    relative_loc: add_parser_loc(0, column, next_pattern.relative_loc),
                    ..next_pattern
                });

                // Remove it from the remaining input
                input = &input[next_pattern_len..];

                // Trim the remaining input
                let trimmed = count_start_whitespaces(input);
                input = &input[trimmed..];

                // Update the column number
                column += next_pattern_len + trimmed;

                // Check the reason why the parser stopped here
                match next_stopped_at {
                    // If it stopped because it was the end of the input, it's time to return the whole collected data
                    PatternParserStoppedAt::End => {
                        break Pattern {
                            relative_loc: ParserLoc { line: 0, col: 0 },
                            repetition: None,
                            is_silent: false,
                            // If the parser stopped on the first pattern because it encountered an union separator, the remaining content
                            // should be put inside an union.
                            // Otherwise, and if no union separator was found during the parsing on the whole rule's content,
                            // a simple suite can be made from the patterns.
                            value: if stopped_at != PatternParserStoppedAt::UnionSep
                                && unions.is_empty()
                            {
                                // Avoid making a whole suite wrapper for a single pattern
                                if patterns.len() == 1 {
                                    break patterns.into_iter().next().unwrap();
                                } else {
                                    RulePatternValue::Suite(patterns)
                                }
                            } else {
                                // Otherwise, terminate the union
                                if !patterns.is_empty() {
                                    unions.push(create_union_child(patterns));
                                }

                                assert!(unions.len() >= 2);

                                RulePatternValue::Union(unions)
                            },
                        };
                    }

                    // If a continuation separator (whitespace) was encountered, just go to the next pattern (= do nothing for now)
                    PatternParserStoppedAt::ContinuationSep => {}

                    // If an union separator (|) was encountered...
                    PatternParserStoppedAt::UnionSep => {
                        // The whole pattern is now considered as an union, given that unions have precedence over everything else

                        // Put the current patterns in the union's members list
                        unions.push(create_union_child(patterns));

                        // Prepare for the next union's member (if any)
                        patterns = vec![];
                    }
                }
            }
        }
    };

    // Update the base location
    if global_pattern.relative_loc.line == 0 {
        global_pattern.relative_loc.col += base_loc.col;
    }
    global_pattern.relative_loc.line += base_loc.line;

    // Success!
    Ok(RuleContent(global_pattern))
}

/// Parse a rule's pattern
/// The success return value is made of the parsed pattern, the consumed input length, and the reason why the parser stopped at this specific symbol
pub fn parse_pattern(
    input: &str,
) -> Result<(Pattern, usize, PatternParserStoppedAt), ParserError> {
    // Left-trim the input
    let (input, trimmed) = trim_start_and_count(input);

    // Parse the first piece (note that the entire pattern may be made of a single one)
    let (first_pattern, first_pattern_len) =
        parse_pattern_piece(input).map_err(|err| add_base_err_loc(0, trimmed, err))?;

    // Remove it from the remaining input
    let input = &input[first_pattern_len..];

    // If the remaining input is empty, the first piece was the whole pattern's content
    if is_finished_line(input) {
        return Ok((
            first_pattern,
            first_pattern_len + trimmed,
            PatternParserStoppedAt::End,
        ));
    }

    let mut chars = input.chars();

    // Get the first character's following the first piece
    let next_char = chars.next().unwrap();

    // If it's a whitespace...
    if next_char.is_whitespace() {
        // It may or may not be followed by an union separator, which can semantically be surrounded by whitespaces

        // The number of characters looked ahead of the end of the first piece
        let mut looked_ahead = 1;

        // Iterate over remaining characters
        for c in chars {
            // Increase the counter
            looked_ahead += 1;

            // Ignore whitespaces
            if c.is_whitespace() {
                continue;
            }
            // If we find an union separator (|)...
            else if c == '|' {
                // Stop right now.
                // The parent function will be in charge of parsing the next members of the union.
                return Ok((
                    first_pattern,
                    first_pattern_len + trimmed + looked_ahead,
                    PatternParserStoppedAt::UnionSep,
                ));
            }
            // If we encounter any other character, it means the first piece was just followed by a whitespace to indicate
            // it was going to be followed by another piece.
            else {
                // Decrease the counter
                looked_ahead -= 1;
                break;
            }
        }

        // Stop here. The parent function will be in charge of parsing the following items.
        Ok((
            first_pattern,
            first_pattern_len + trimmed + looked_ahead,
            PatternParserStoppedAt::ContinuationSep,
        ))
    }
    // If we find an union separator (|)...
    else if next_char == '|' {
        // Stop right now.
        // The parent function will be in charge of parsing the next members of the union.
        Ok((
            first_pattern,
            first_pattern_len + trimmed + 1,
            PatternParserStoppedAt::UnionSep,
        ))
    }
    // If we find any other character...
    else {
        // That's an error, as the content should not end right now.
        Err(ParserError::new(
            ParserLoc::new(0, first_pattern_len + trimmed),
            1,
            ParserErrorContent::ExpectedPatternSeparatorOrEndOfLine,
            Some("adding another pattern to the suite requires a whitespace, or a vertical bar (|) for an union")
        ))
    }
}

/// Parse a rule's piece, which means a single value
///
/// This function's success return value is the parsed piece and the consumed input length
fn parse_pattern_piece(input: &str) -> Result<(Pattern, usize), ParserError> {
    // Determine if the piece is silent
    let (trimmed, is_silent) = parse_rule_pattern_silence(input);
    let input = &input[trimmed..];

    let (value, len) =
    // Check if the value is a constant string
    if let Some((string, len)) = singles::cst_string(input)? {
        (RulePatternValue::CstString(string), len)
    }
    // Check if the value is a rule's name
    else if let Some((name, len)) = singles::rule_name(input)? {
        (RulePatternValue::Rule(name), len)
    }
    // Check if the value is a group (`(...)`)
    else if let Some((group, len)) = singles::group(input)? {
        (RulePatternValue::Group(group), len)
    }
    // If it's none of the above, it is syntax error
    else {
        return Err(ParserError::new(
            ParserLoc::new(0, 0),
            0,
            ParserErrorContent::ExpectedRuleContent,
            Some(match input.chars().next() {
                Some('\'') => "strings require double quotes",
                Some(_) => "You may either open a group with '(', a string with '\"', or specify a rule's name",
                None => "you need to provide a rule pattern, such as a group, a string or a rule's name"
            })
        ));
    };

    // Get the piece's repetition model (* + ?) following it
    let repetition = input.chars().nth(len).and_then(PatternRepetition::parse);

    // Success!
    Ok((
        Pattern {
            relative_loc: ParserLoc { line: 0, col: 0 },
            value,
            is_silent,
            repetition,
        },
        trimmed + len + if repetition.is_some() { 1 } else { 0 },
    ))
}

/// Parse a possibly silent pattern beginning
///
/// If the pattern is indicated to be non-capturing (silent), the consumed size will be returned with the `true` value
pub fn parse_rule_pattern_silence(input: &str) -> (usize, bool) {
    if input.starts_with("_:") {
        (2, true)
    } else {
        (0, false)
    }
}

/// Reason by the [pattern parser](`parse_rule_pattern`) stopped at a specific moment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternParserStoppedAt {
    End,
    ContinuationSep,
    UnionSep,
}

/// Syntax tree generated by [the compiler](`parse_peg`)
#[derive(Debug)]
pub struct PegSyntaxTree<'a> {
    rules: HashMap<&'a str, RuleContent<'a>>,
}

impl<'a> PegSyntaxTree<'a> {
    /// Get the rules of the syntax tree (the map's keys are the rules' name)
    pub fn rules(&self) -> &HashMap<&'a str, RuleContent<'a>> {
        &self.rules
    }

    /// Get the rule's main rule
    pub fn main_rule(&self) -> &RuleContent<'a> {
        &self.rules[GRAMMAR_ENTRYPOINT_RULE]
    }
}

/// A rule's content, parsed by the [`parse_rule_content`] function
#[derive(Debug)]
pub struct RuleContent<'a>(pub(crate) Pattern<'a>);

impl<'a> RuleContent<'a> {
    pub fn pattern(&self) -> &Pattern<'a> {
        &self.0
    }
}

/// A rule's pattern, parsed by the [`parse_rule_pattern`] function
#[derive(Debug)]
pub struct Pattern<'a> {
    /// Pattern's beginning, relative to its parent
    relative_loc: ParserLoc,

    /// Repetition model
    repetition: Option<PatternRepetition>,

    /// Is the pattern silent?
    is_silent: bool,

    /// The pattern's value
    value: RulePatternValue<'a>,
}

impl<'a> Pattern<'a> {
    /// Get the pattern's beginning, relatively to its parent
    pub fn relative_loc(&self) -> ParserLoc {
        self.relative_loc
    }

    /// Get the pattern's repetition model
    pub fn repetition(&self) -> Option<PatternRepetition> {
        self.repetition
    }

    /// Is the pattern silent?
    pub fn is_silent(&self) -> bool {
        self.is_silent
    }

    /// Get the pattern's value
    pub fn value(&self) -> &RulePatternValue<'a> {
        &self.value
    }
}

/// [Rule pattern](`RulePattern`)'s repetition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternRepetition {
    // The pattern can be provided any number of times
    Any,

    // The pattern can be provided one or more times
    OneOrMore,

    // The pattern can be provided or not
    Optional,
}

impl PatternRepetition {
    /// Check if a symbol is a valid repetition symbol
    pub fn is_valid_symbol(symbol: char) -> bool {
        symbol == '*' || symbol == '+' || symbol == '?'
    }

    /// Try to parse a repetition symbol
    pub fn parse(symbol: char) -> Option<Self> {
        match symbol {
            '*' => Some(Self::Any),
            '+' => Some(Self::OneOrMore),
            '?' => Some(Self::Optional),
            _ => None,
        }
    }

    /// Get the symbol associated to a rule's repetition model
    pub fn symbol(self) -> char {
        match self {
            Self::Any => '*',
            Self::OneOrMore => '+',
            Self::Optional => '?',
        }
    }
}

/// A single [`RulePattern`]'s value, indicating which content it must match
#[derive(Debug)]
pub enum RulePatternValue<'a> {
    /// Match a constant string
    CstString(&'a str),

    /// Match using another rule's content
    Rule(&'a str),

    /// Match using a group (will match on the inner pattern)
    Group(Rc<Pattern<'a>>),

    /// Match a suite of patterns
    Suite(Vec<Pattern<'a>>),

    /// Match one of the provided patterns
    /// Evaluation is performed in order, and the first matching pattern will be used
    Union(Vec<Pattern<'a>>),
}

/// Location in the input grammar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParserLoc {
    /// Line number
    line: usize,

    /// Column number
    col: usize,
}

impl ParserLoc {
    /// Create a new location
    pub(crate) fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }

    /// Get the location's line number
    pub fn line(&self) -> usize {
        self.line
    }

    /// Get the location's column number
    pub fn col(&self) -> usize {
        self.col
    }
}
