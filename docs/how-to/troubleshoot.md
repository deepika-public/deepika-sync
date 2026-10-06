# Dépanner, sauvegarder, récupérer

Un problème pendant une session se lit d'abord dans le journal ; ce guide dit où il est,
comment le lire, puis passe en revue les situations courantes et ce qu'il faut en faire.

## Les journaux

Chaque session écrit **`<racine>/.collab/daemon.log`**, une ligne JSON par événement, plus
une sauvegarde `daemon.log.1` au-delà de 8 Mo. C'est vrai quel que soit le lanceur, y
compris quand un autre programme démarre le daemon et qu'aucun terminal ne montre sa
sortie. Le chemin est affiché au démarrage (`Journal : …`). `.collab` n'est jamais
partagé avec les pairs.

Pour lire un journal, `scripts/diagnose.py` regroupe les points d'attention par code,
chacun avec sa signification et le geste à faire :

```bash
python3 scripts/diagnose.py ~/coffre/equipe              # synthèse ; sortie 0, 1 (avertissements), 2 (pas de journal)
python3 scripts/diagnose.py ~/coffre/equipe --timeline   # déroulé complet, pour dater une bascule
python3 scripts/diagnose.py /tmp/alice/docs /tmp/bob/docs --timeline   # plusieurs racines, fusionnées par horodatage
python3 scripts/diagnose.py ~/coffre/equipe --timeline --bundle rapport.txt   # compte rendu à transmettre
```

| Niveau | Sens |
| --- | --- |
| `ERROR` | Une garantie est en jeu. À traiter. |
| `WARN` | Collision de chemin tranchée, message refusé, service indisponible. Normal si vous l'avez provoqué ; à comprendre sinon. |
| `INFO` | Le fil de la session : ouverture, pairs, imports disque. |
| `DEBUG` | Le détail : découverte mDNS, échecs d'appel, connexions WebSocket. |

Le fichier va jusqu'à `DEBUG`, le terminal s'arrête à `INFO`. Pour changer :
`RUST_LOG=deepika_sync=debug` agit sur le terminal, `DEEPIKA_SYNC_LOG=deepika_sync::projection=trace`
sur le fichier, module par module (`workspace`, `projection`, `transport`, `discovery`,
`ws`, `lsp`).

Le texte des notes, les invitations et les capacités ne sont jamais journalisés. Les
chemins et identifiants de documents, si : relire un journal avant de l'envoyer.

Pour un essai à deux, chacun produit son rapport `--bundle` ; un conflit doit apparaître
des deux côtés avec le même document et des horodatages cohérents. Un avertissement
présent d'un seul côté mérite une explication.

## Situations courantes

**`session_already_running` au démarrage.** Un autre daemon tient déjà la racine. Le
verrou appartient au processus : il disparaît avec lui, rien à nettoyer. Avec Obsidian,
c'est le plugin qui gère ce cas.

**`root_already_belongs_to_another_session`.** Ce dossier a déjà rejoint une autre
session. Utiliser une autre racine ; ne pas effacer `.collab` pour forcer le
rattachement.

**Le daemon a planté.** Le relancer avec la même commande. SQLite récupère ses
transactions, les documents sont rechargés puis resynchronisés avec les pairs, et les
éditeurs se reconnectent en renvoyant ce que leur réplique a de plus, sans doublon.
`deepika-sync status` et `deepika-sync documents` confirment.

**Deux fichiers `note.md` et `note (conflit 3f9a1c2e).md`.** En rejoignant, votre note
différait de celle de la session ; rien n'a été écrasé, les deux existent partout, celle
dont l'identifiant est le plus grand a été renommée (ce n'est pas forcément la vôtre).
Comparer (`diff`), reporter à la main ce qu'il faut garder dans `note.md` — chaque frappe
se synchronise — puis supprimer le fichier `(conflit …)`, ou le renommer pour le garder.
Les copies identiques, elles, ne produisent aucun conflit : la redondante est retirée.
La règle complète est dans [rejoindre une session](../reference/join-behavior.md).

**Une modification faite hors de l'éditeur a disparu.** Tant qu'un éditeur tient un
document, le daemon n'importe pas les changements disque de ce fichier et ne l'écrit pas ;
à la fermeture, il écrit l'état partagé, qui remplace ce qu'un outil externe
(`git checkout`, `sed -i`) avait mis entre-temps. Refaire la modification dans l'éditeur,
ou fermer l'éditeur avant de lancer l'outil.

**Un fichier a été supprimé par erreur.** La suppression s'est propagée. Recréer le
fichier depuis une sauvegarde : ce sera un nouveau document au même chemin.

**Renommer ou supprimer sans éditeur.** Pendant une session, agir sur le fichier, le
daemon suit. Daemon arrêté, par identifiant :

```bash
deepika-sync documents --root ~/docs                         # relever l'UUID
deepika-sync rename '<UUID>' nouveau/chemin.md --root ~/docs
deepika-sync delete '<UUID>' --root ~/docs
```

## Sauvegarder et restaurer

Sauvegarder : fermer les éditeurs, arrêter le daemon et attendre sa sortie, puis copier
les Markdown **et tout `.collab`** hors de la racine. Ne pas copier `state.sqlite` seul
pendant l'exécution : le WAL peut contenir des transactions.

Restaurer : daemon arrêté, remettre ensemble Markdown et `.collab` de **ce même pair**.
Une sauvegarde ancienne n'a pas les opérations récentes ; les pairs à jour les renverront
à la reconnexion. Ne jamais cloner `.collab` pour créer un autre utilisateur : il contient
la clé privée du pair.

Ce qui survit à un redémarrage, et comment, est expliqué dans
[durabilité](../explanation/durability.md).
