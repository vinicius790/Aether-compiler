# Gramática Aether 0.1

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
AssignStmt  ::= Expr "=" Expr ";"
ExprStmt    ::= Expr ";"
ReturnStmt  ::= "return" Expr? ";"
BreakStmt   ::= "break" ";"
ContinueStmt::= "continue" ";"
IfStmt      ::= "if" Expr Block ("else" (IfStmt | Block))?
WhileStmt   ::= "while" Expr Block
ForStmt     ::= "for" Ident "in" Expr ".." Expr Block

Expr        ::= Or

Or          ::= And ("||" And)*
And         ::= Cmp ("&&" Cmp)*
Cmp         ::= Add (("=="|"!="|"<"|"<="|">"|">=") Add)*
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

Literais inteiros: dígitos decimais, cabem em `i64` na análise léxica.
Literais flutuantes: `Digitos "." Digitos (("e"|"E") ("+"|"-")? Digitos)?`.
