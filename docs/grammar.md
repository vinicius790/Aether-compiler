# Gramática Aether 0.3

Notação: EBNF. Tokens em maiúsculas ou entre aspas. `Ident` não é palavra-chave.

```
Program     ::= Item*

Item        ::= "pub"? (FnItem | StructItem | EnumItem | ExternItem) | UseItem

FnItem      ::= "fn" Ident "(" ParamList? ")" ("->" Type)? Block
ExternItem  ::= "extern" "fn" Ident "(" ParamList? ")" ("->" Type)? ";"
StructItem  ::= "struct" Ident "{" FieldList? "}"
UseItem     ::= "use" StringLit ";"
EnumItem    ::= "enum" Ident "{" VariantList? "}"
VariantList ::= Variant ("," Variant)* ","?
Variant     ::= Ident ("(" Type ("," Type)* ","? ")")?

ParamList   ::= Param ("," Param)* ","?
Param       ::= Ident ":" Type
FieldList   ::= Field ("," Field)* ","?
Field       ::= Ident ":" Type

Type        ::= "i32" | "i64" | "f64" | "bool" | "string" | "unit" | "char"
              | Ident
              | "[" Type ";" IntLit "]"
              | "(" ")"
              | TupleType
TupleType   ::= "(" Type ("," Type)+ ","? ")"

Block       ::= "{" Stmt* Expr? "}"

Stmt        ::= LetStmt | LetTupleStmt | IfStmt | IfLetStmt | MatchStmt
              | WhileStmt | ForStmt
              | ReturnStmt | BreakStmt | ContinueStmt | YieldStmt
              | Block
              | AssignStmt | ExprStmt

LetStmt     ::= "let" "mut"? Ident (":" Type)? ("=" Expr)? ";"
LetTupleStmt::= "let" "mut"? "(" Ident ("," Ident)+ ","? ")" "=" Expr ";"
AssignStmt  ::= Expr AssignOp Expr ";"
AssignOp    ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
              | "&=" | "|=" | "^=" | "<<=" | ">>="
ExprStmt    ::= Expr ";"
ReturnStmt  ::= "return" Expr? ";"
BreakStmt   ::= "break" ";"
ContinueStmt::= "continue" ";"
YieldStmt   ::= "yield" ";"
IfStmt      ::= "if" Expr Block ("else" (IfStmt | Block))?
IfLetStmt   ::= "if" "let" Pattern "=" Expr Block ("else" (IfStmt | Block))?
MatchStmt   ::= "match" Expr "{" MatchArm* "}"
MatchArm    ::= Pattern "=>" Block ","?
Pattern     ::= "_" | Ident
              | Ident "::" Ident ("(" Pattern ("," Pattern)* ","? ")")?
              | "-"? IntLit | "true" | "false" | CharLit | StringLit
WhileStmt   ::= "while" Expr Block
ForStmt     ::= "for" Ident "in" Expr ".." Expr Block

Expr        ::= Or

Or          ::= And ("||" And)*
And         ::= Eq ("&&" Eq)*
Eq          ::= Cmp (("=="|"!=") Cmp)*
Cmp         ::= BitOr (("<"|"<="|">"|">=") BitOr)*
BitOr       ::= BitXor ("|" BitXor)*
BitXor      ::= BitAnd ("^" BitAnd)*
BitAnd      ::= Shift ("&" Shift)*
Shift       ::= Add (("<<"|">>") Add)*
Add         ::= Mul (("+"|"-") Mul)*
Mul         ::= Cast (("*"|"/"|"%") Cast)*
Cast        ::= Unary ("as" Type)*
Unary       ::= ("-"|"!") Unary | Postfix
Postfix     ::= Primary (Call | Index | Field)*
Call        ::= "(" ArgList? ")"
Index       ::= "[" Expr "]"
Field       ::= "." (Ident | IntLit)
ArgList     ::= Expr ("," Expr)* ","?

Primary     ::= Ident StructLit?
              | Ident "::" Ident ("(" ArgList? ")")?
              | IntLit | FloatLit | StringLit | CharLit
              | "true" | "false"
              | "(" Expr? ")"
              | TupleExpr
              | "[" ArgList? "]"

TupleExpr   ::= "(" Expr ("," Expr)+ ","? ")"

StructLit   ::= "{" FieldInit ("," FieldInit)* ","? "}"
FieldInit   ::= Ident ":" Expr
```

Comentários: `//` até o fim da linha; `/* ... */` não aninhados.

`pub` é aceite e registado na AST mas não tem efeito em 0.3 (visibilidade
não verificada). `UseItem` importa os itens de outro ficheiro; o caminho é
relativo ao ficheiro corrente e `.ae` é opcional (ver language.md,
“Módulos”).
`t.0.1` lê-se como `(t.0).1`: o léxico produz o float `0.1`, que o parser
divide em dois índices. Nos padrões de `match`, `_` é o identificador `_`;
os sub-padrões de uma variante são só nomes ou `_`. `match` só é instrução
(`Stmt`), nunca expressão.

Precedência (do mais frouxo ao mais apertado): `||` 1, `&&` 2, `== !=` 3,
`< <= > >=` 4, `|` 5, `^` 6, `&` 7, `<< >>` 8, `+ -` 9, `* / %` 10, `as` 11,
prefixos 12. Todos os infixos associam à esquerda. `a op= b` é açúcar para
`a = a op b`.

```
IntLit      ::= DecLit | "0x" HexDigit ("_"? HexDigit)* | "0b" BinDigit ("_"? BinDigit)*
              | "0o" OctDigit ("_"? OctDigit)*
DecLit      ::= Digit ("_"? Digit)*
FloatLit    ::= DecLit "." DecLit (("e"|"E") ("+"|"-")? Digit+)?
Escape      ::= "\n" | "\t" | "\r" | "\0" | "\\" | "\"" | "\'" | "\u{" HexDigit{1,6} "}"
```

O léxico aceita `_` em qualquer posição após o primeiro dígito (também
`0x_ff`); o valor tem de caber em `i64`. Os tokens `&&`, `||`, `->`, `>=`,
`<=`, `>>=` e `<<=` são sempre preferidos ao prefixo mais curto (maximal
munch); não há genéricos, pelo que `>>` nunca fecha dois `>`.
