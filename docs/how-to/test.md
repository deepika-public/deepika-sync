# Tester, mesurer

Depuis la racine du dépôt, sous Linux avec Rust 1.95+, outils C et Python 3 :

```bash
python3 scripts/check-docs.py     # la doc : pas de version ni de lien figé, pas de lien mort
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
python3 scripts/test-two-peers.py
python3 scripts/test-mesh.py
```

Les tests Rust, par fichier :

| Fichier | Ce qu'il tient |
| --- | --- |
| `tests/convergence.rs` | Offsets UTF-16 (accents, emoji), fusion d'imports disque concurrents, collision de chemin (textes différents / identiques), recréation d'un chemin supprimé, extinction des échos, rattrapage à la libération d'un bail, compaction et rechargement |
| `tests/workspace.rs` | CRUD, baux, adoption d'un document distant |
| `tests/storage.rs` | Rechargement SQLite, index lisible pendant que la session est verrouillée, migration de l'ancien schéma d'index, refus des symlinks internes |
| `tests/projection.rs` | Événements disque → CRDT et matérialisation CRDT → disque ; rattrapage au démarrage de ce qui a changé pendant l'arrêt |
| `tests/lifecycle.rs` | Le binaire lancé avec `--exit-with-parent` vit tant que son entrée standard est tenue, s'arrête proprement quand elle se ferme |
| `tests/ws.rs` | Handshake y-sync, routage par URL, refus des origines navigateur, relais des mises à jour et de l'awareness sans écho |
| `tests/lsp.rs` | Serveur LSP branché sur un vrai WebSocket : édition incrémentale et `workspace/applyEdit`, caractères hors BMP compris |
| `tests/security.rs` | Mauvaise preuve ⇒ aucun document ; le `Hello` ne contient jamais la capacité |
| `tests/discovery.rs` | Nom de service mDNS dérivé de la capacité |
| `tests/reconnect.rs` | L'invité rappelle un hôte redémarré ; relancé sans invitation, il retrouve l'hôte qu'il a mémorisé |
| `tests/preview.rs` | Plan de ce que rejoindre changera (dossier neuf, reprise, identique) ; manifeste obtenu sans rien écrire, refusé sans la capacité |

Pour cibler un fichier : `cargo test --locked --test convergence`.

`test-two-peers.py` lance deux vrais daemons Iroh, vérifie la création et l'édition
croisées par le disque, puis interroge `documents` **pendant** que le daemon tourne et
après son arrêt. `test-mesh.py` fait de même à trois pairs. Ils utilisent leurs propres
dossiers temporaires, sans Obsidian ni relais public.

La carte des situations concurrentes et le test qui couvre chacune sont dans
[cas d'édition concurrente](../reference/concurrency-cases.md). Un succès est une sortie
sans erreur de chaque commande. Ces tests locaux ne valident ni deux réseaux physiques
séparés par un NAT, ni une coupure électrique. Les scénarios dans de vraies fenêtres
Obsidian vivent dans le dépôt du plugin.

Après un essai manuel, `scripts/diagnose.py` lit le journal ; voir [dépanner](troubleshoot.md).

## Mesurer la latence et les ressources

```bash
cargo build --release --locked
python3 scripts/benchmark.py --binary target/release/deepika-sync --size 1024 --edits 30 --output /tmp/deepika-sync-benchmark.json
```

Le script lance ses propres pairs Iroh directs dans des dossiers temporaires et mesure le
délai entre une écriture disque chez un pair et son apparition chez les autres, plus CPU,
RSS, écritures disque et compteurs QUIC. Le JSON contient les paramètres et le SHA-256 du
binaire. Ce délai inclut le debounce du watcher ; il ne mesure ni le rendu d'un éditeur
ni un réseau WAN.

Varier avec `--size 102400`, `--peers 5`, `--history 100`, ou `--size 32 --files 1000` ;
`--settle 10` attend dix secondes avant la mesure au repos. Comparer deux versions exige
le même matériel, les mêmes paramètres et le même profil de build ; des échantillons peu
nombreux ne disent rien des latences extrêmes. Les grosses notes restent coûteuses :
l'import disque calcule un diff caractère par caractère, et tous les documents sont en
mémoire.
