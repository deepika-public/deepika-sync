# Observer une collision de chemin à la connexion, sans éditeur

Vous allez connecter deux daemons dont les dossiers contiennent une note de même nom,
et vérifier qu'aucune des deux versions n'est remplacée. Prérequis : Linux, projet
construit avec `cargo build --locked`. Commandes à exécuter depuis la racine du dépôt.

## Préparer deux dossiers

Dans un premier terminal :

```bash
ESSAI=$(mktemp -d /tmp/deepika-sync-join-XXXXXX)
mkdir "$ESSAI/alice" "$ESSAI/bob"
printf 'Version Alice\n' > "$ESSAI/alice/note.md"
printf 'Version Bob\n'   > "$ESSAI/bob/note.md"
printf 'pareil\n' | tee "$ESSAI/alice/commun.md" > "$ESSAI/bob/commun.md"
printf 'Dossier de test : %s\n' "$ESSAI"
./target/debug/deepika-sync share "$ESSAI/alice"
```

Gardez ce terminal ouvert. Copiez le dossier affiché et le code après `Code d'invitation :`.

Dans un deuxième terminal :

```bash
read -r ESSAI      # coller le dossier, puis Entrée
read -r INVITE     # coller le code complet, puis Entrée
./target/debug/deepika-sync join "$INVITE" --root "$ESSAI/bob" --ws-port 4445
```

`--ws-port` évite que les deux daemons du même poste se disputent le port 4444.

Avant de rejoindre, la commande annonce ce qui attend le dossier de Bob :

```text
La session contient 2 note(s).

Même nom, contenu différent — les deux versions sont gardées, l'une sous « nom (conflit …).md » (1) :
  note.md

1 note(s) déjà identiques : rien à faire.

Rejoindre la session ? [o/N]
```

Répondre `n` : le dossier de Bob est intact, sans même un `.collab`. Relancer la commande
et répondre `o`. Chaque daemon affiche alors deux avertissements `path_collision`.

## Vérifier

Dans un troisième terminal — les daemons tournent toujours :

```bash
read -r ESSAI
ls "$ESSAI/alice" "$ESSAI/bob"
./target/debug/deepika-sync documents --root "$ESSAI/bob"
```

Les deux dossiers contiennent exactement les mêmes trois fichiers :

| Fichier | Contenu | Pourquoi |
| --- | --- | --- |
| `note.md` | une des deux versions | le document à l'UUID le plus petit garde le nom |
| `note (conflit xxxxxxxx).md` | l'autre version | textes différents : rien n'est fusionné ni écrasé |
| `commun.md` | `pareil` | textes identiques : la copie redondante a été retirée (`"deleted": true` dans `documents`) |

Quelle version garde le nom dépend d'UUID aléatoires ; l'important est que le choix soit
le même chez Alice et chez Bob, sans qu'ils se soient concertés.

## Terminer

`Ctrl+C` dans les deux premiers terminaux. Les fichiers restent lisibles une fois les
daemons arrêtés.

Pour fusionner les deux versions dans votre propre dossier, suivez le
[dépannage](../how-to/troubleshoot.md). Pour comprendre ce choix, lisez
[rejoindre une session](../reference/join-behavior.md).
