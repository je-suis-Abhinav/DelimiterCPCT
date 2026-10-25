mod recoverykind_test_8_y {
    mod _parser_ {
        #![allow(clippy::type_complexity)]
        #![allow(clippy::unnecessary_wraps)]
        #![deny(unsafe_code)]
        #[allow(unused_imports)]
        use super::*;
        const __GRM_DATA: &[u8] = &[
            2u8, 2u8, 1u8, 94u8, 0u8, 0u8, 5u8, 83u8, 116u8, 97u8, 114u8, 116u8, 104u8,
            109u8, 1u8, 0u8, 1u8, 0u8, 1u8, 0u8, 1u8, 0u8, 2u8, 1u8, 2u8, 0u8, 1u8, 0u8,
            1u8, 2u8, 1u8, 1u8, 1u8, 0u8, 2u8, 1u8, 0u8, 2u8, 0u8, 0u8, 1u8, 111u8,
            111u8, 0u8, 2u8, 0u8, 0u8, 1u8, 0u8, 0u8, 0u8, 0u8, 2u8, 0u8, 0u8, 0u8, 0u8,
            0u8,
        ];
        const __STABLE_DATA: &[u8] = &[
            2u8, 0u8, 1u8, 1u8, 0u8, 2u8, 1u8, 0u8, 2u8, 1u8, 253u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 64u8, 1u8, 2u8, 2u8, 1u8, 3u8, 2u8, 0u8, 0u8, 2u8, 0u8, 4u8,
            1u8, 13u8, 2u8, 1u8, 253u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 32u8, 2u8,
            0u8, 0u8, 4u8, 1u8, 1u8, 2u8, 1u8, 0u8, 2u8, 1u8, 1u8, 2u8, 1u8, 0u8,
        ];
        const __SERIALISATION_FORMAT: ::lrpar::ctbuilder::SerialisationFormat = ::lrpar::ctbuilder::SerialisationFormat::VariableSizedInteger;
        fn __lrpar_parser_data() -> &'static ::lrpar::ParserData<u16> {
            static DATA: ::std::sync::OnceLock<::lrpar::ParserData<u16>> = ::std::sync::OnceLock::new();
            DATA.get_or_init(|| {
                match __SERIALISATION_FORMAT {
                    ::lrpar::ctbuilder::SerialisationFormat::FixedSizeInteger => {
                        ::lrpar::ctbuilder::_reconstitute(
                            __GRM_DATA,
                            __STABLE_DATA,
                            ::lrpar::ctbuilder::wincode::config::Configuration::default()
                                .with_fixint_encoding(),
                        )
                    }
                    ::lrpar::ctbuilder::SerialisationFormat::VariableSizedInteger => {
                        ::lrpar::ctbuilder::_reconstitute(
                            __GRM_DATA,
                            __STABLE_DATA,
                            ::lrpar::ctbuilder::wincode::config::Configuration::default()
                                .with_varint_encoding(),
                        )
                    }
                    _ => {
                        panic!(
                            "Parser source was generated using unknown `SerialisationFormat`: {:?}",
                            "serialisation_format"
                        )
                    }
                }
            })
        }
        #[allow(dead_code)]
        pub fn parse<'lexer, 'input: 'lexer>(
            lexer: &'lexer dyn ::lrpar::NonStreamingLexer<
                'input,
                lrpar::test_utils::TestLexerTypes,
            >,
        ) -> ::std::vec::Vec<
            ::lrpar::LexParseError<u16, lrpar::test_utils::TestLexerTypes>,
        > {
            parse_with_recovery_kind(lexer, ::lrpar::RecoveryKind::CPCTPlus)
        }
        #[allow(dead_code)]
        pub fn parse_with_recovery_kind<'lexer, 'input: 'lexer>(
            lexer: &'lexer dyn ::lrpar::NonStreamingLexer<
                'input,
                lrpar::test_utils::TestLexerTypes,
            >,
            recovery_kind: ::lrpar::RecoveryKind,
        ) -> ::std::vec::Vec<
            ::lrpar::LexParseError<u16, lrpar::test_utils::TestLexerTypes>,
        > {
            let __data = __lrpar_parser_data();
            let grm = __data.grm();
            let stable = __data.stable();
            ::lrpar::RTParserBuilder::new(grm, stable)
                .recoverer(recovery_kind)
                .parse_map(lexer, &|_| (), &|_, _| ())
                .1
        }
        #[allow(dead_code)]
        pub const R_START: u16 = 1;
        const __GT_EPP: &[::std::option::Option<&str>] = &[::std::option::Option::None];
        /// Return the %epp entry for token `tidx` (where `None` indicates \"the token has no
        /// pretty-printed value\"). Panics if `tidx` doesn't exist.
        #[allow(dead_code)]
        pub fn token_epp<'a>(
            tidx: ::cfgrammar::TIdx<u16>,
        ) -> ::std::option::Option<&'a str> {
            __GT_EPP[usize::from(tidx)]
        }
    }
    #[allow(unused_imports)]
    pub use _parser_::*;
    #[allow(unused_imports)]
    use ::lrpar::Lexeme;
}
/* CACHE INFORMATION "BUILD_TIME = \"2026-10-07T19:00:41.215694400Z\" DERIVED_MOD_NAME = \"recoverykind_test_8_y\" ENCODING_CONFIG = :: lrpar :: ctbuilder :: SerialisationFormat :: VariableSizedInteger GRAMMAR_PATH = \"C:\\\\Users\\\\Abhinav Ojha\\\\OneDrive\\\\Desktop\\\\VIT\\\\DelimiterCPCT\\\\DelimiterCPCT\\\\vendor\\\\lrpar\\\\target\\\\debug\\\\build\\\\lrpar-699bd9e912907567\\\\out/recoverykind_test_8.y\" MOD_NAME = :: std :: option :: Option :: None RECOVERER = :: lrpar :: RecoveryKind :: CPCTPlus YACC_KIND = :: cfgrammar :: yacc :: YaccKind :: Original (:: cfgrammar :: yacc :: YaccOriginalActionKind :: NoAction) ERROR_ON_CONFLICTS = true SHOW_WARNINGS = true WARNINGS_ARE_ERRORS = true RUST_EDITION = :: lrpar :: RustEdition :: Rust2021 RULE_IDS_MAP = [(0usize , \"<unknown>\") ,] VISIBILITY = :: lrpar :: Visibility :: Private" */
