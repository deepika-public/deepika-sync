# Installer, mettre à jour

Le daemon est un seul exécutable, `deepika-sync`, sans dépendance à installer : SQLite est
embarqué. Linux x86_64 (glibc 2.34 ou plus) est validé ; macOS ne l'est pas encore, voir
[compatibilité](../reference/compatibility.md).

Avec Obsidian, le binaire va **dans le dossier du plugin**, qui le trouve tout seul : suivre
alors le [guide d'installation du plugin](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/docs/how-to/install.md),
qui installe les deux d'un coup.

## Depuis la dernière release

Sur la [page des releases](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/deepika-sync/-/releases)
(il faut avoir accès au projet), télécharger dans un même dossier vide l'archive
`deepika-sync-<version>-x86_64-unknown-linux-gnu.tar.gz` et `SHA256SUMS`, sans les renommer.
Puis, dans un terminal ouvert dans ce dossier :

```bash
(
  set -e
  sha256sum -c SHA256SUMS
  tar -xzf deepika-sync-*-x86_64-unknown-linux-gnu.tar.gz
  mkdir -p "$HOME/.local/bin"
  install -m 755 deepika-sync-*-x86_64-unknown-linux-gnu/deepika-sync "$HOME/.local/bin/deepika-sync"
  "$HOME/.local/bin/deepika-sync" --version
)
```

Résultat attendu : **OK**, puis `deepika-sync <version>`. Le bloc s'arrête si la vérification
échoue. Une erreur mentionnant `GLIBC` signifie un système trop ancien : la version requise
est dans le `BUILD.json` extrait.

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
