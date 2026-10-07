use lrlex::CTLexerBuilder;

fn main() {
    CTLexerBuilder::new()
        .lrpar_config(|ctp| ctp.grammar_in_src_dir("expr.y").unwrap())
        .lexer_in_src_dir("expr.l")
        .unwrap()
        .build()
        .unwrap();
}
