# Mapa do catálogo de repositórios → Aether

O ficheiro de ideias cobre dezenas de repos. Este compilador **não** é
todos eles. A tabela diz o que foi absorvido e o que seria um repo novo.

| § | Ideia | Relação com Aether |
|---|--------|-------------------|
| 3.x | Compiladores no título da secção | Este repo |
| 5.5 | Script + VM para games | `docs/scripting.md`, `profile`, natives |
| 5.10 | Replay determinístico | `digest`, `--seed` |
| 6.1 | Subset C | Linguagem própria, não C; mesma cadeia |
| 6.6 | WASM | Não; a VM é o sandbox |
| 6.7 | Profiler | `aether profile` |
| 6.2–6.5 | Git, containers, Kafka, HNSW | Fora |
| 1–2 | DB, emulador, Raft, mesh | Fora |
| 4, 13–15 | Autograd, RAG, GNN, LDM | Fora |
| 8–12 | UE5, Unity, Godot, Blender, Maya | Fora |

Regra: se a ideia precisa de um tick de jogo, de uma GPU ou de um
cluster, não entra neste ZIP.
