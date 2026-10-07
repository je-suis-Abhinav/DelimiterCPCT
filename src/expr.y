%grmtools{yacckind: Grmtools}
%start Expr
%%
Expr -> Result<u64, ()>:
      Expr '+' Term { Ok($1? + $3?) }
    | Term { $1 }
    ;

Term -> Result<u64, ()>:
      Term '*' Factor { Ok($1? * $3?) }
    | Factor { $1 }
    ;

Factor -> Result<u64, ()>:
      '(' Expr ')' { $2 }
    | 'INT'
      {
          let lexeme = $1.map_err(|_| ())?;
          parse_int($lexer.span_str(lexeme.span()))
      }
    ;
%%
fn parse_int(s: &str) -> Result<u64, ()> {
    s.parse::<u64>().map_err(|_| ())
}
