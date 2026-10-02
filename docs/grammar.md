# Gramática Aether 0.2

Notação: EBNF. Tokens em maiúsculas ou entre aspas. `Ident` não é palavra-chave.

```
Program     ::= Item*

Item        ::= FnItem | StructItem | ExternItem

FnItem      ::= "fn" Ident "(" ParamList? ")" ("->" Type)? Block
ExternItem  ::= "extern" "fn" Ident "(" ParamList? ")" ("->" Type)? ";"
StructItem  ::= "struct" Ident "{" FieldList? "}"

ParamList   ::= Param ("," Param)* ","?
Param       ::= Ident ":" Type
FieldList   ::= Field ("," Field)* ","?
Field       ::= Ident ":" Type

Type        ::= "i32" | "i64" | "f64" | "bool" | "string" | "unit" | "char"
              | Ident
              | "[" Type ";" IntLit "]"
              | "(" ")"

Block       ::= "{" Stmt* Expr? "}"

Stmt        ::= LetStmt | IfStmt | WhileStmt | ForStmt
              | ReturnStmt | BreakStmt | ContinueStmt
              | Block
              | AssignStmt | ExprStmt

LetStmt     ::= "let" "mut"? Ident (":" Type)? ("=" Expr)? ";"
AssignStmt  ::= Expr AssignOp Expr ";"
AssignOp    ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
              | "&=" | "|=" | "^=" | "<<=" | ">>="
ExprStmt    ::= Expr ";"
ReturnStmt  ::= "return" Expr? ";"
BreakStmt   ::= "break" ";"
ContinueStmt::= "continue" ";"
IfStmt      ::= "if" Expr Block ("else" (IfStmt | Block))?
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
Field       ::= "." Ident
ArgList     ::= Expr ("," Expr)* ","?

Primary     ::= Ident StructLit?
              | IntLit | FloatLit | StringLit | CharLit
              | "true" | "false"
              | "(" Expr? ")"
              | "[" ArgList? "]"

StructLit   ::= "{" FieldInit ("," FieldInit)* ","? "}"
FieldInit   ::= Ident ":" Expr
```

Comentários: `//` até o fim da linha; `/* ... */` não aninhados.

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
