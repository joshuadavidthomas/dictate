use std::cmp::Reverse;

use crate::transcription::RawTranscript;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessedDictation {
    text: String,
}

impl ProcessedDictation {
    fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationMode {
    Raw,
    Literal,
    Message,
    Email,
    Note,
    Technical,
    Command,
}

impl DictationMode {
    const fn default_spoken_formatting(self) -> SpokenFormatting {
        match self {
            Self::Raw | Self::Literal => SpokenFormatting::Disabled,
            Self::Message | Self::Email | Self::Note | Self::Technical | Self::Command => {
                SpokenFormatting::PunctuationAndLines
            }
        }
    }

    const fn applies_phrase_replacements(self) -> bool {
        match self {
            Self::Raw | Self::Literal => false,
            Self::Message | Self::Email | Self::Note | Self::Technical | Self::Command => true,
        }
    }

    const fn removes_fillers(self) -> bool {
        match self {
            Self::Raw | Self::Literal => false,
            Self::Message | Self::Email | Self::Note | Self::Technical | Self::Command => true,
        }
    }

    const fn capitalizes_sentences(self) -> bool {
        match self {
            Self::Raw | Self::Literal => false,
            Self::Message | Self::Email | Self::Note | Self::Technical | Self::Command => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpokenFormatting {
    Disabled,
    PunctuationOnly,
    PunctuationAndLines,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictationContext {
    mode: DictationMode,
    spoken_formatting: SpokenFormatting,
    dictionary: CustomDictionary,
    replacement_rules: Vec<ReplacementRule>,
}

impl DictationContext {
    pub fn new(mode: DictationMode) -> Self {
        Self {
            mode,
            spoken_formatting: mode.default_spoken_formatting(),
            dictionary: CustomDictionary::default(),
            replacement_rules: Vec::new(),
        }
    }

    pub fn mode(&self) -> DictationMode {
        self.mode
    }

    pub fn with_spoken_formatting(mut self, spoken_formatting: SpokenFormatting) -> Self {
        self.spoken_formatting = spoken_formatting;
        self
    }

    pub fn with_dictionary(mut self, dictionary: CustomDictionary) -> Self {
        self.dictionary = dictionary;
        self
    }

    pub fn with_replacement_rule(mut self, rule: ReplacementRule) -> Self {
        self.replacement_rules.push(rule);
        self
    }

    pub fn with_replacement_rules(mut self, rules: Vec<ReplacementRule>) -> Self {
        self.replacement_rules = rules;
        self
    }
}

impl Default for DictationContext {
    fn default() -> Self {
        Self::new(DictationMode::Message)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CustomDictionary {
    terms: Vec<DictionaryTerm>,
}

impl CustomDictionary {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_entries<I, S, W>(entries: I) -> Self
    where
        I: IntoIterator<Item = (S, W)>,
        S: Into<String>,
        W: Into<String>,
    {
        entries
            .into_iter()
            .fold(Self::empty(), |dictionary, (spoken, written)| {
                dictionary.with_term(spoken, written)
            })
    }

    pub fn with_term(mut self, spoken: impl Into<String>, written: impl Into<String>) -> Self {
        self.terms.push(DictionaryTerm {
            spoken: spoken.into(),
            written: written.into(),
        });
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DictionaryTerm {
    spoken: String,
    written: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplacementRule {
    spoken: String,
    replacement: String,
}

impl ReplacementRule {
    pub fn new(spoken: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            spoken: spoken.into(),
            replacement: replacement.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DictationFormatter;

impl DictationFormatter {
    pub fn format(&self, raw: RawTranscript, context: &DictationContext) -> ProcessedDictation {
        let normalized = normalize_whitespace(raw.as_str());
        if normalized.is_empty() || context.mode == DictationMode::Raw {
            return ProcessedDictation::new(normalized);
        }

        let tokens = tokenize(&normalized);
        let phrase_replacements = phrase_replacements(context);
        let mut output = OutputText::default();
        let mut index = 0;

        while index < tokens.len() {
            if context.mode.removes_fillers()
                && let Some(consumed) = filler_at(&tokens, index)
            {
                index += consumed_with_following_punctuation(&tokens, index, consumed);
                continue;
            }

            if context.mode.applies_phrase_replacements()
                && let Some(replacement) = replacement_at(&phrase_replacements, &tokens, index)
            {
                let consumed = replacement.spoken.len();
                let written = replacement_text(&tokens, index, consumed, &replacement.written);
                output.push_text(&written);
                index += consumed;
                continue;
            }

            if let Some((consumed, command)) =
                formatting_command_at(&tokens, index, context.spoken_formatting)
            {
                output.push_formatting(command);
                index += consumed_with_following_punctuation(&tokens, index, consumed);
                continue;
            }

            output.push_text(tokens[index].raw);
            index += 1;
        }

        let mut text = output.finish();
        if context.mode.capitalizes_sentences() {
            text = capitalize_sentences(&text);
        }

        ProcessedDictation::new(text)
    }
}

#[derive(Clone, Debug)]
struct Token<'a> {
    raw: &'a str,
    key: String,
    leading_punctuation: &'a str,
    trailing_punctuation: &'a str,
}

impl Token<'_> {
    fn has_leading_punctuation(&self) -> bool {
        !self.leading_punctuation.is_empty()
    }

    fn has_trailing_punctuation(&self) -> bool {
        !self.trailing_punctuation.is_empty()
    }

    fn is_punctuation_only(&self) -> bool {
        self.key.is_empty()
    }
}

#[derive(Clone, Debug)]
struct PhraseReplacement {
    spoken: Vec<String>,
    written: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FormattingCommand {
    Punctuation(&'static str),
    LineBreak(LineBreak),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineBreak {
    NewLine,
    NewParagraph,
}

#[derive(Default)]
struct OutputText {
    text: String,
}

impl OutputText {
    fn push_text(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }

        if needs_space_before(&self.text, value) {
            self.text.push(' ');
        }
        self.text.push_str(value);
    }

    fn push_formatting(&mut self, command: FormattingCommand) {
        match command {
            FormattingCommand::Punctuation(mark) => self.push_punctuation(mark),
            FormattingCommand::LineBreak(line_break) => self.push_line_break(line_break),
        }
    }

    fn push_punctuation(&mut self, mark: &str) {
        trim_trailing_spaces(&mut self.text);
        self.text.push_str(mark);
    }

    fn push_line_break(&mut self, line_break: LineBreak) {
        trim_trailing_spaces(&mut self.text);
        if self.text.is_empty() {
            return;
        }

        match line_break {
            LineBreak::NewLine => self.text.push('\n'),
            LineBreak::NewParagraph => self.text.push_str("\n\n"),
        }
    }

    fn finish(self) -> String {
        normalize_output_spacing(self.text)
    }
}

fn phrase_replacements(context: &DictationContext) -> Vec<PhraseReplacement> {
    let mut replacements = Vec::new();

    for rule in &context.replacement_rules {
        push_phrase_replacement(&mut replacements, &rule.spoken, &rule.replacement);
    }

    for term in &context.dictionary.terms {
        push_phrase_replacement(&mut replacements, &term.spoken, &term.written);
    }

    if context.mode == DictationMode::Technical {
        for (spoken, written) in TECHNICAL_TERMS {
            push_phrase_replacement(&mut replacements, spoken, written);
        }
    }

    replacements.sort_by_key(|replacement| Reverse(replacement.spoken.len()));
    replacements
}

fn push_phrase_replacement(replacements: &mut Vec<PhraseReplacement>, spoken: &str, written: &str) {
    let spoken = phrase_tokens(spoken);
    if spoken.is_empty() {
        return;
    }

    replacements.push(PhraseReplacement {
        spoken,
        written: written.to_string(),
    });
}

fn replacement_at<'a>(
    replacements: &'a [PhraseReplacement],
    tokens: &[Token<'_>],
    index: usize,
) -> Option<&'a PhraseReplacement> {
    replacements
        .iter()
        .find(|replacement| matches_phrase(tokens, index, &replacement.spoken))
}

fn formatting_command_at(
    tokens: &[Token<'_>],
    index: usize,
    spoken_formatting: SpokenFormatting,
) -> Option<(usize, FormattingCommand)> {
    match spoken_formatting {
        SpokenFormatting::Disabled => None,
        SpokenFormatting::PunctuationOnly => punctuation_command_at(tokens, index),
        SpokenFormatting::PunctuationAndLines => {
            line_command_at(tokens, index).or_else(|| punctuation_command_at(tokens, index))
        }
    }
}

fn line_command_at(tokens: &[Token<'_>], index: usize) -> Option<(usize, FormattingCommand)> {
    if matches_words(tokens, index, &["new", "paragraph"]) {
        Some((2, FormattingCommand::LineBreak(LineBreak::NewParagraph)))
    } else if matches_words(tokens, index, &["new", "line"]) {
        Some((2, FormattingCommand::LineBreak(LineBreak::NewLine)))
    } else {
        None
    }
}

fn punctuation_command_at(
    tokens: &[Token<'_>],
    index: usize,
) -> Option<(usize, FormattingCommand)> {
    if matches_words(tokens, index, &["question", "mark"]) {
        Some((2, FormattingCommand::Punctuation("?")))
    } else if matches_words(tokens, index, &["exclamation", "mark"]) {
        Some((2, FormattingCommand::Punctuation("!")))
    } else if matches_words(tokens, index, &["full", "stop"]) {
        Some((2, FormattingCommand::Punctuation(".")))
    } else if matches_words(tokens, index, &["comma"]) {
        Some((1, FormattingCommand::Punctuation(",")))
    } else if matches_words(tokens, index, &["period"]) {
        Some((1, FormattingCommand::Punctuation(".")))
    } else if matches_words(tokens, index, &["colon"]) {
        Some((1, FormattingCommand::Punctuation(":")))
    } else if matches_words(tokens, index, &["semicolon"]) {
        Some((1, FormattingCommand::Punctuation(";")))
    } else {
        None
    }
}

fn filler_at(tokens: &[Token<'_>], index: usize) -> Option<usize> {
    if matches_words(tokens, index, &["you", "know"]) {
        Some(2)
    } else if matches_words(tokens, index, &["um"])
        || matches_words(tokens, index, &["uh"])
        || matches_words(tokens, index, &["er"])
        || matches_words(tokens, index, &["ah"])
        || matches_words(tokens, index, &["hmm"])
    {
        Some(1)
    } else {
        None
    }
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    text.split_whitespace()
        .map(|raw| {
            let (leading_punctuation, core, trailing_punctuation) = split_token_punctuation(raw);
            let key = spoken_key(core);

            Token {
                raw,
                key,
                leading_punctuation,
                trailing_punctuation,
            }
        })
        .collect()
}

fn split_token_punctuation(raw: &str) -> (&str, &str, &str) {
    let Some(core_start) = raw.find(|character: char| !character.is_ascii_punctuation()) else {
        return (raw, "", "");
    };
    let core_end_start = raw
        .rfind(|character: char| !character.is_ascii_punctuation())
        .expect("core_start proves at least one non-punctuation character");
    let core_end = core_end_start
        + raw[core_end_start..]
            .chars()
            .next()
            .expect("core_end_start points at a character")
            .len_utf8();

    (
        &raw[..core_start],
        &raw[core_start..core_end],
        &raw[core_end..],
    )
}

fn phrase_tokens(phrase: &str) -> Vec<String> {
    phrase
        .split_whitespace()
        .map(spoken_key)
        .filter(|word| !word.is_empty())
        .collect()
}

fn matches_phrase(tokens: &[Token<'_>], index: usize, phrase: &[String]) -> bool {
    span_allows_match(tokens, index, phrase.len())
        && phrase.iter().enumerate().all(|(offset, word)| {
            tokens
                .get(index + offset)
                .is_some_and(|token| token.key == *word)
        })
}

fn matches_words(tokens: &[Token<'_>], index: usize, words: &[&str]) -> bool {
    span_allows_match(tokens, index, words.len())
        && words.iter().enumerate().all(|(offset, word)| {
            tokens
                .get(index + offset)
                .is_some_and(|token| token.key == *word)
        })
}

fn span_allows_match(tokens: &[Token<'_>], index: usize, len: usize) -> bool {
    if len <= 1 {
        return true;
    }

    (0..len).all(|offset| {
        tokens.get(index + offset).is_some_and(|token| {
            (offset == 0 || !token.has_leading_punctuation())
                && (offset + 1 == len || !token.has_trailing_punctuation())
        })
    })
}

fn replacement_text(tokens: &[Token<'_>], index: usize, consumed: usize, written: &str) -> String {
    let first = &tokens[index];
    let last = &tokens[index + consumed - 1];
    let mut text = String::with_capacity(
        first.leading_punctuation.len() + written.len() + last.trailing_punctuation.len(),
    );

    text.push_str(first.leading_punctuation);
    text.push_str(written);
    text.push_str(last.trailing_punctuation);
    text
}

fn consumed_with_following_punctuation(
    tokens: &[Token<'_>],
    index: usize,
    consumed: usize,
) -> usize {
    let mut total = consumed;
    while tokens
        .get(index + total)
        .is_some_and(Token::is_punctuation_only)
    {
        total += 1;
    }
    total
}

fn spoken_key(word: &str) -> String {
    word.trim_matches(|character: char| character.is_ascii_punctuation())
        .to_ascii_lowercase()
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_output_spacing(text: String) -> String {
    let mut normalized = text
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    while normalized.contains("\n\n\n") {
        normalized = normalized.replace("\n\n\n", "\n\n");
    }

    normalized
}

fn needs_space_before(current: &str, next: &str) -> bool {
    if current.is_empty() || current.ends_with(char::is_whitespace) {
        return false;
    }

    !starts_with_closing_punctuation(next)
}

fn starts_with_closing_punctuation(text: &str) -> bool {
    matches!(
        text.chars().next(),
        Some(',' | '.' | '?' | '!' | ':' | ';' | ')' | ']' | '}')
    )
}

fn trim_trailing_spaces(text: &mut String) {
    while text.ends_with(' ') {
        text.pop();
    }
}

fn capitalize_sentences(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut capitalize_next = true;

    for character in text.chars() {
        if capitalize_next && character.is_alphabetic() {
            for uppercase in character.to_uppercase() {
                output.push(uppercase);
            }
            capitalize_next = false;
            continue;
        }

        output.push(character);

        if matches!(character, '.' | '?' | '!' | '\n') {
            capitalize_next = true;
        }
    }

    output
}

const TECHNICAL_TERMS: &[(&str, &str)] = &[
    ("gpui", "GPUI"),
    ("g p u i", "GPUI"),
    ("sherpa onnx", "sherpa-onnx"),
    ("sherpa dash onnx", "sherpa-onnx"),
    ("wayland", "Wayland"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn format(input: &str, context: DictationContext) -> String {
        DictationFormatter
            .format(RawTranscript::new(input), &context)
            .as_str()
            .to_string()
    }

    #[test]
    fn message_punctuation_matches_snapshot() {
        insta::assert_snapshot!(
            format(
                "hey there comma can you look at this question mark",
                DictationContext::new(DictationMode::Message),
            ),
            @"Hey there, can you look at this?"
        );
    }

    #[test]
    fn email_paragraphs_match_snapshot() {
        insta::assert_snapshot!(
            format(
                "hello Josh comma new paragraph thanks for the update period new paragraph best comma Dictate",
                DictationContext::new(DictationMode::Email),
            ),
            @r###"
Hello Josh,

Thanks for the update.

Best, Dictate
"###
        );
    }

    #[test]
    fn technical_terms_match_snapshot() {
        insta::assert_snapshot!(
            format(
                "gpui uses sherpa onnx on wayland period",
                DictationContext::new(DictationMode::Technical),
            ),
            @"GPUI uses sherpa-onnx on Wayland."
        );
    }

    #[test]
    fn filler_removal_matches_snapshot() {
        insta::assert_snapshot!(
            format(
                "um hello uh world period",
                DictationContext::new(DictationMode::Message),
            ),
            @"Hello world."
        );
    }

    #[test]
    fn literal_and_raw_modes_preserve_spoken_commands_snapshot() {
        let literal = format(
            "write the words new paragraph comma",
            DictationContext::new(DictationMode::Literal),
        );
        let raw = format(
            "  write   comma  exactly  ",
            DictationContext::new(DictationMode::Raw),
        );

        insta::assert_snapshot!(
            format!("literal:\n{literal}\n\nraw:\n{raw}"),
            @r###"
literal:
write the words new paragraph comma

raw:
write comma exactly
"###
        );
    }

    #[test]
    fn raw_mode_only_trims_and_normalizes_whitespace() {
        let context = DictationContext::new(DictationMode::Raw);

        assert_eq!(
            format("  hello   comma   world  ", context),
            "hello comma world"
        );
    }

    #[test]
    fn message_mode_applies_safe_spoken_punctuation() {
        assert_eq!(
            format(
                "hey there comma can you look at this question mark",
                DictationContext::new(DictationMode::Message),
            ),
            "Hey there, can you look at this?",
        );
    }

    #[test]
    fn email_mode_formats_new_paragraphs() {
        assert_eq!(
            format(
                "hello comma new paragraph thanks period",
                DictationContext::new(DictationMode::Email),
            ),
            "Hello,\n\nThanks.",
        );
    }

    #[test]
    fn literal_mode_preserves_command_words() {
        assert_eq!(
            format(
                "write the words new paragraph",
                DictationContext::new(DictationMode::Literal),
            ),
            "write the words new paragraph",
        );
    }

    #[test]
    fn literal_mode_can_enable_punctuation_without_line_commands() {
        let context = DictationContext::new(DictationMode::Literal)
            .with_spoken_formatting(SpokenFormatting::PunctuationOnly);

        assert_eq!(
            format("write comma then new paragraph", context),
            "write, then new paragraph",
        );
    }

    #[test]
    fn technical_mode_preserves_project_terms() {
        assert_eq!(
            format(
                "gpui uses sherpa onnx on wayland",
                DictationContext::new(DictationMode::Technical),
            ),
            "GPUI uses sherpa-onnx on Wayland",
        );
    }

    #[test]
    fn custom_dictionary_replaces_spoken_terms() {
        let dictionary = CustomDictionary::empty().with_term("gee pee you eye", "GPUI");
        let context = DictationContext::new(DictationMode::Technical).with_dictionary(dictionary);

        assert_eq!(format("i use gee pee you eye", context), "I use GPUI");
    }

    #[test]
    fn replacement_rules_expand_snippets() {
        let context = DictationContext::new(DictationMode::Email)
            .with_replacement_rule(ReplacementRule::new("insert signature", "Best,\nJosh"));

        assert_eq!(
            format("thanks period insert signature", context),
            "Thanks. Best,\nJosh"
        );
    }

    #[test]
    fn technical_mode_preserves_trailing_punctuation_on_replacements() {
        assert_eq!(
            format(
                "I prefer GPUI.",
                DictationContext::new(DictationMode::Technical),
            ),
            "I prefer GPUI.",
        );
    }

    #[test]
    fn technical_mode_preserves_clause_punctuation_on_replacements() {
        assert_eq!(
            format(
                "Gpui, sherpa onnx, and Wayland.",
                DictationContext::new(DictationMode::Technical),
            ),
            "GPUI, sherpa-onnx, and Wayland.",
        );
    }

    #[test]
    fn punctuation_commands_do_not_cross_sentence_boundaries() {
        assert_eq!(
            format(
                "That's a good question. Mark will answer.",
                DictationContext::new(DictationMode::Message),
            ),
            "That's a good question. Mark will answer.",
        );
    }

    #[test]
    fn line_commands_do_not_cross_sentence_boundaries() {
        assert_eq!(
            format(
                "Something new. Paragraph two is next.",
                DictationContext::new(DictationMode::Message),
            ),
            "Something new. Paragraph two is next.",
        );
    }

    #[test]
    fn punctuation_commands_do_not_cross_standalone_sentence_boundaries() {
        assert_eq!(
            format(
                "That's a good question . Mark will answer.",
                DictationContext::new(DictationMode::Message),
            ),
            "That's a good question. Mark will answer.",
        );
    }

    #[test]
    fn punctuation_commands_drop_asr_attached_punctuation() {
        assert_eq!(
            format(
                "Hello comma, world period.",
                DictationContext::new(DictationMode::Message),
            ),
            "Hello, world.",
        );
    }

    #[test]
    fn custom_dictionary_preserves_trailing_punctuation() {
        let dictionary = CustomDictionary::empty().with_term("gee pee you eye", "GPUI");
        let context = DictationContext::new(DictationMode::Email).with_dictionary(dictionary);

        assert_eq!(format("I use gee pee you eye.", context), "I use GPUI.");
    }

    #[test]
    fn filler_removal_drops_attached_punctuation() {
        assert_eq!(
            format(
                "Um, hello world.",
                DictationContext::new(DictationMode::Message)
            ),
            "Hello world.",
        );
    }

    #[test]
    fn line_commands_drop_asr_attached_punctuation() {
        assert_eq!(
            format(
                "Hello comma New paragraph. Thanks period",
                DictationContext::new(DictationMode::Email),
            ),
            "Hello,\n\nThanks.",
        );
    }

    #[test]
    fn multi_token_fillers_do_not_cross_punctuation_boundaries() {
        assert_eq!(
            format(
                "You, know the answer.",
                DictationContext::new(DictationMode::Message),
            ),
            "You, know the answer.",
        );
    }

    #[test]
    fn multi_token_fillers_do_not_cross_standalone_punctuation_boundaries() {
        assert_eq!(
            format(
                "You . know the answer.",
                DictationContext::new(DictationMode::Message),
            ),
            "You. Know the answer.",
        );
    }

    #[test]
    fn commands_drop_following_standalone_punctuation() {
        assert_eq!(
            format(
                "Hello comma , world period .",
                DictationContext::new(DictationMode::Message),
            ),
            "Hello, world.",
        );
    }

    #[test]
    fn commands_drop_following_standalone_punctuation_runs() {
        assert_eq!(
            format(
                "Hello comma , . world period .",
                DictationContext::new(DictationMode::Message),
            ),
            "Hello, world.",
        );
    }

    #[test]
    fn fillers_drop_following_standalone_punctuation_runs() {
        assert_eq!(
            format(
                "Um , . hello world.",
                DictationContext::new(DictationMode::Message),
            ),
            "Hello world.",
        );
    }

    #[test]
    fn non_literal_modes_remove_fillers() {
        assert_eq!(
            format(
                "um hello uh world period",
                DictationContext::new(DictationMode::Message),
            ),
            "Hello world.",
        );
    }
}
