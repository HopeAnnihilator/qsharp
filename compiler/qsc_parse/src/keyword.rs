// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use enum_iterator::Sequence;
use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Sequence)]
pub enum Keyword {
    Adj,
    Adjoint,
    AdjointUpper,
    And,
    Apply,
    As,
    Auto,
    Body,
    Borrow,
    Controlled,
    ControlledUpper,
    Ctl,
    Distribute,
    Elif,
    Else,
    Export,
    Fail,
    False,
    Fixup,
    For,
    Function,
    If,
    In,
    Internal,
    Intrinsic,
    Invert,
    Is,
    Let,
    Mutable,
    Namespace,
    Newtype,
    Not,
    One,
    Open,
    Operation,
    Or,
    PauliI,
    PauliX,
    PauliY,
    PauliZ,
    Repeat,
    Return,
    Slf,
    Set,
    True,
    Underscore,
    Until,
    Use,
    While,
    Within,
    Zero,
}

impl Keyword {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Adj => "Adj",
            Self::Adjoint => "adjoint",
            Self::AdjointUpper => "Adjoint",
            Self::And => "and",
            Self::Apply => "apply",
            Self::As => "as",
            Self::Auto => "auto",
            Self::Body => "body",
            Self::Borrow => "borrow",
            Self::Controlled => "controlled",
            Self::ControlledUpper => "Controlled",
            Self::Ctl => "Ctl",
            Self::Distribute => "distribute",
            Self::Elif => "elif",
            Self::Else => "else",
            Self::Export => "export",
            Self::Fail => "fail",
            Self::False => "false",
            Self::Fixup => "fixup",
            Self::For => "for",
            Self::Function => "function",
            Self::If => "if",
            Self::In => "in",
            Self::Internal => "internal",
            Self::Intrinsic => "intrinsic",
            Self::Invert => "invert",
            Self::Is => "is",
            Self::Let => "let",
            Self::Mutable => "mutable",
            Self::Namespace => "namespace",
            Self::Newtype => "newtype",
            Self::Not => "not",
            Self::One => "One",
            Self::Open => "open",
            Self::Operation => "operation",
            Self::Or => "or",
            Self::PauliI => "PauliI",
            Self::PauliX => "PauliX",
            Self::PauliY => "PauliY",
            Self::PauliZ => "PauliZ",
            Self::Repeat => "repeat",
            Self::Return => "return",
            Self::Slf => "self",
            Self::Set => "set",
            Self::True => "true",
            Self::Underscore => "_",
            Self::Until => "until",
            Self::Use => "use",
            Self::While => "while",
            Self::Within => "within",
            Self::Zero => "Zero",
        }
    }
}

impl Display for Keyword {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Keyword {
    type Err = ();

    // This is a hot function. Use a match expression so that the Rust compiler
    // can optimize the string comparisons better, and order the cases by
    // frequency in Q# so that fewer comparisons are needed on average.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "let" => Ok(Self::Let),
            "operation" => Ok(Self::Operation),
            "is" => Ok(Self::Is),
            "in" => Ok(Self::In),
            "and" => Ok(Self::And),
            "for" => Ok(Self::For),
            "function" => Ok(Self::Function),
            "open" => Ok(Self::Open),
            "if" => Ok(Self::If),
            "return" => Ok(Self::Return),
            "Adj" => Ok(Self::Adj),
            "Controlled" => Ok(Self::ControlledUpper),
            "controlled" => Ok(Self::Controlled),
            "Ctl" => Ok(Self::Ctl),
            "set" => Ok(Self::Set),
            "use" => Ok(Self::Use),
            "as" => Ok(Self::As),
            "not" => Ok(Self::Not),
            "true" => Ok(Self::True),
            "Zero" => Ok(Self::Zero),
            "One" => Ok(Self::One),
            "namespace" => Ok(Self::Namespace),
            "mutable" => Ok(Self::Mutable),
            "internal" => Ok(Self::Internal),
            "PauliZ" => Ok(Self::PauliZ),
            "false" => Ok(Self::False),
            "PauliX" => Ok(Self::PauliX),
            "PauliI" => Ok(Self::PauliI),
            "Adjoint" => Ok(Self::AdjointUpper),
            "adjoint" => Ok(Self::Adjoint),
            "apply" => Ok(Self::Apply),
            "intrinsic" => Ok(Self::Intrinsic),
            "or" => Ok(Self::Or),
            "elif" => Ok(Self::Elif),
            "fail" => Ok(Self::Fail),
            "else" => Ok(Self::Else),
            "within" => Ok(Self::Within),
            "body" => Ok(Self::Body),
            "newtype" => Ok(Self::Newtype),
            "invert" => Ok(Self::Invert),
            "distribute" => Ok(Self::Distribute),
            "auto" => Ok(Self::Auto),
            "self" => Ok(Self::Slf),
            "while" => Ok(Self::While),
            "until" => Ok(Self::Until),
            "repeat" => Ok(Self::Repeat),
            "fixup" => Ok(Self::Fixup),
            // The next three were not found or measured
            // in the standard library for priority order.
            "PauliY" => Ok(Self::PauliY),
            "borrow" => Ok(Self::Borrow),
            "export" => Ok(Self::Export),
            "_" => Ok(Self::Underscore),
            _ => Err(()),
        }
    }
}
