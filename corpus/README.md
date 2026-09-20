# Corpus

Programas `.ae` pequenos usados como regressão extra e como semente
mental do fuzzer de formato. Não são o corpus greybox (esse vive em
memória durante `aether fuzz --kind greybox`).

Convenção: `cNNN.ae` deve ter `fn main() -> i32` e terminar em tempo
curto. Gerados com valores determinísticos (índice no nome).
