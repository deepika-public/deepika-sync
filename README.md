# deepika-sync

Édition collaborative de dossiers Markdown, sans serveur. Un daemon par dossier, sur
chaque poste : il garde les notes sous forme de documents CRDT (Yrs), les écrit en Markdown
ordinaire sur le disque, et se synchronise avec les autres postes de pair à pair (Iroh
QUIC, chiffré de bout en bout, relais public seulement sur demande). Les fichiers restent
lisibles sans lui.

Les éditeurs s'y branchent par deux standards, et ne parlent jamais aux pairs :

- **LSP** sur stdio (`deepika-sync lsp`) pour Neovim, Helix, VS Code, Zed, Emacs ;
- **WebSocket y-sync** pour Obsidian (plugin [dot-sync](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync)) et tout client Yjs ;
- et un **[client web](web/README.md)** publié sur <https://deepika-public.gitlab.io/deepika-obsidian-toolbox/deepika-sync>, qui rejoint une session depuis un navigateur, sans rien installer.

## Installer

Avec Obsidian, le plugin dot-sync installe le daemon d'un clic, dans ses réglages.
Sinon, télécharger l'archive de sa plateforme (Linux x86_64 ou arm64, macOS Apple Silicon
ou Intel) de la [dernière release](https://github.com/deepika-public/deepika-sync/releases) et `SHA256SUMS` dans un même dossier, puis :

```bash
sha256sum --ignore-missing -c SHA256SUMS && tar -xzf deepika-sync-*.tar.gz
install -m 755 deepika-sync-*/deepika-sync ~/.local/bin/deepika-sync && deepika-sync --version
```

Depuis les sources : `cargo install --locked --path .` (Rust 1.95+). Le détail, dont macOS
et la mise à jour, est dans [installer](docs/how-to/install.md).

## Utiliser

```bash
deepika-sync share ~/docs --relay                          # l'hôte : affiche l'invitation
deepika-sync join '<INVITATION>' --root ~/docs --relay     # l'invité : montre ce qui va changer, demande, rejoint
```

Le daemon reste au premier plan ; `Ctrl-C` l'arrête, relancer la même commande reprend.
Une invitation donne accès à tout le dossier, en écriture : l'envoyer en privé. Le
[tutoriel](docs/tutorials/first-session.md) déroule une première session à deux.

## Ce qu'il garantit, et ce qu'il ne fait pas

- Une frappe n'est annoncée aux pairs qu'après son écriture durable dans SQLite ; après une
  coupure, un plantage ou une mise en veille, tout se resynchronise seul.
- Deux notes de même nom créées chacune de son côté ne sont jamais fusionnées ni écrasées :
  les deux versions restent, côte à côte.
- Ce qui change dans le dossier pendant que le daemon est arrêté est rattrapé au démarrage.
- Seuls les fichiers `.md` sont suivis. Linux est validé ; macOS est expérimental (construit et testé par la CI, essais réels en cours) ; Windows n'est pas encore pris en charge.
  Pas de droits fins : une invitation vaut pour toute la session, sans révocation.

## Documentation

[Index](docs/README.md) · [CLI](docs/reference/cli.md) ·
[Versions, compatibilité, plateformes](docs/reference/compatibility.md) ·
[Dépanner](docs/how-to/troubleshoot.md) · [Architecture](docs/explanation/architecture.md) ·
[Tester](docs/how-to/test.md) · [Publier une release](docs/how-to/release.md).
