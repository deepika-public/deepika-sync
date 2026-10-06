# Première session à deux, en ligne de commande

Deux personnes, chacune un terminal, à distance. L'**hôte** partage un dossier, l'**invité**
le rejoint. Dix minutes. Avec Obsidian, suivre plutôt le
[tutoriel du plugin](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/docs/tutorials/two-users.md),
qui fait la même chose par clic droit.

## 1. Installer — les deux

Suivre [installer](../how-to/install.md) depuis la dernière release ; `deepika-sync --version`
doit répondre.

## 2. L'hôte partage

```bash
mkdir -p ~/partage && echo "Bonjour" > ~/partage/premiere.md
deepika-sync share ~/partage --relay
```

`--relay` est ce qui permet à quelqu'un hors du réseau local de vous joindre ; les
échanges restent chiffrés de bout en bout. Le daemon reste au premier plan et affiche :

```
Session ouverte
Racine : /home/alice/partage
Journal : /home/alice/partage/.collab/daemon.log
Code d'invitation : eyJ2ZXJzaW9uIjox…
WebSocket : ws://127.0.0.1:4444
```

Envoyer la ligne qui suit `Code d'invitation :` à l'invité, **par une messagerie privée** :
elle donne accès à tout le dossier, en lecture et en écriture. Laisser le terminal ouvert.

## 3. L'invité rejoint

Dans un dossier vide ou dans le sien :

```bash
mkdir -p ~/partage
deepika-sync join 'eyJ2ZXJzaW9uIjox…' --root ~/partage --relay
```

`join` se connecte à l'hôte et affiche **ce qui va changer dans le dossier** — fichiers
créés, notes de même nom gardées côte à côte, notes envoyées à l'hôte — puis demande
`Rejoindre la session ? [o/N]`. Rien n'est écrit avant `o`. Les notes de l'hôte arrivent.

## 4. Écrire

Chacun ouvre `premiere.md` dans l'éditeur de son choix et écrit ; l'autre voit le fichier
changer dans la seconde. Pour des curseurs en direct et des frappes fusionnées caractère
par caractère, brancher l'éditeur au daemon : Neovim ou Helix par [LSP](../reference/editor-protocol.md),
Obsidian par son plugin, ou un navigateur par le [client web](../../web/README.md) que
l'hôte lie avec `https://deepika-public.gitlab.io/deepika-obsidian-toolbox/deepika-sync/#<invitation>`.

Dans un autre terminal, `deepika-sync status --root ~/partage` décrit la session et
`deepika-sync documents --root ~/partage` liste les notes. `Ctrl-C` dans le terminal du
daemon arrête la session ; relancer la même commande la reprend, et les deux côtés se
resynchronisent seuls.

## Ça ne marche pas ?

| Symptôme | À vérifier |
| --- | --- |
| `inviter_unreachable` chez l'invité | L'hôte tourne toujours ? `--relay` **des deux côtés**, et chez l'hôte **avant** de partager ? Invitation collée en entier, entre guillemets simples ? |
| `session_already_running` | Un daemon tient déjà ce dossier ; l'arrêter, ou choisir un autre dossier |
| Rien n'arrive après avoir rejoint | [Dépanner](../how-to/troubleshoot.md) : lire `.collab/daemon.log` des deux côtés |

Une invitation créée sans `--relay` ne sert à rien à distance : l'hôte arrête, relance avec
`--relay`, et renvoie la nouvelle.
