# Installer, mettre à jour

Le daemon est un seul exécutable, `deepika-sync`, sans dépendance à installer : SQLite est
embarqué. Binaires publiés pour Linux x86_64 et arm64 (statiques, toute distribution) et
macOS Apple Silicon et Intel (expérimental) ; voir les [plateformes](../reference/compatibility.md#plateformes).

**Avec Obsidian, rien à faire ici** : le plugin [dot-sync](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/docs/how-to/install.md)
installe le daemon d'un clic, dans ses réglages, et vérifie sa somme SHA-256.

## Depuis la dernière release

Sur la [page des releases GitHub](https://github.com/deepika-public/deepika-sync/releases), télécharger dans un même dossier vide `SHA256SUMS` et
l'archive de sa plateforme, sans la renommer :

| Système | Archive |
| --- | --- |
| Linux x86_64 | `deepika-sync-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 | `deepika-sync-<version>-aarch64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `deepika-sync-<version>-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `deepika-sync-<version>-x86_64-apple-darwin.tar.gz` |

Puis, dans un terminal ouvert dans ce dossier :

```bash
(
  set -e
  sha256sum --ignore-missing -c SHA256SUMS      # macOS : shasum -a 256 --ignore-missing -c SHA256SUMS
  tar -xzf deepika-sync-*.tar.gz
  mkdir -p "$HOME/.local/bin"
  install -m 755 deepika-sync-*/deepika-sync "$HOME/.local/bin/deepika-sync"
  "$HOME/.local/bin/deepika-sync" --version
)
```

Résultat attendu : **OK** pour l'archive téléchargée, puis `deepika-sync <version>`. Le bloc
s'arrête si la vérification échoue. Sur macOS, une archive ouverte depuis le navigateur peut
être bloquée par Gatekeeper (binaire non notarisé) : `xattr -d com.apple.quarantine
"$HOME/.local/bin/deepika-sync"` la débloque. Chaque archive est aussi attestée :
`gh attestation verify <archive> --repo deepika-public/deepika-sync` prouve qu'elle a été
construite par la CI depuis ce dépôt.

La release GitLab garde l'archive Linux x86_64 liée à la glibc
(`…-x86_64-unknown-linux-gnu.tar.gz`, glibc 2.34 ou plus), utilisée par la CI du plugin.

Si `deepika-sync` n'est pas trouvé ensuite, ajouter `~/.local/bin` au `PATH` :

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Depuis les sources

Prérequis : Rust 1.95 ou plus, un compilateur C et les outils de compilation. Ni Node.js
ni Obsidian. Depuis la racine du dépôt :

```bash
cargo install --locked --path .
deepika-sync --version
```

Le binaire est dans `~/.cargo/bin`, à ajouter au `PATH` si nécessaire.

## Premier usage

```bash
deepika-sync share /chemin/vers/docs
```

Le daemon reste au premier plan et affiche l'invitation à transmettre. Chez l'invité, dans
son propre dossier :

```bash
deepika-sync join '<INVITATION>' --root /chemin/vers/docs
```

Le tutoriel [première session à deux](../tutorials/first-session.md) déroule ces deux
commandes pas à pas ; la [CLI](../reference/cli.md) décrit toutes les options.

## Mettre à jour

1. Fermer les éditeurs connectés, puis arrêter chaque daemon (`Ctrl-C` ou `SIGTERM`) et
   attendre la fin du processus. Un daemon lancé par Obsidian s'arrête avec lui.
2. Par prudence, [sauvegarder](troubleshoot.md#sauvegarder-et-restaurer) les Markdown et
   tout `.collab` ; ne jamais renommer ni supprimer `.collab`, il contient l'identité du
   pair et la session.
3. Installer le nouveau binaire comme ci-dessus, au même emplacement.
4. Relancer chaque racine avec la même commande qu'avant ; `deepika-sync status --root <racine>`
   confirme la version.

Les pairs d'une session peuvent être mis à jour l'un après l'autre tant que le
[protocole réseau](../reference/compatibility.md) ne change pas ; quand il change, tous
ensemble. Ce que chaque version fait à l'ouverture d'une base existante est dans le
[journal des versions](../reference/compatibility.md#journal-des-versions). Un retour à un
binaire plus ancien exige de restaurer la sauvegarde et abandonne ce qui a été accepté
depuis.
