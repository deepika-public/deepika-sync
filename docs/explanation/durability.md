# Ce qui survit à un redémarrage

L'état privé réside sous `<racine>/.collab/`. SQLite embarqué (`rusqlite`) utilise
`journal_mode=WAL` et `synchronous=FULL` : une transaction engagée survit à une coupure
de courant, dans la limite des garanties du filesystem local et du matériel. Ne pas
placer la base active sur un filesystem réseau.

## Le trajet d'une modification

Toute modification — frappe d'un éditeur, import disque, mise à jour d'un pair,
renommage — emprunte le même chemin (`Workspace::mutate`) :

1. La transaction Yrs est appliquée au document en mémoire, sous verrou.
2. Si elle n'a rien changé (mise à jour déjà connue), elle s'arrête là : ni écriture,
   ni annonce. C'est ce qui éteint les échos dans un maillage comportant un cycle.
3. La mise à jour binaire et la ligne d'index du document sont écrites dans **une
   même transaction SQLite**, hors des threads asynchrones (`spawn_blocking`).
4. L'événement est publié : les pairs, les éditeurs et la projection disque le reçoivent.

L'annonce suit donc l'écriture durable. Une mise à jour dont une dépendance manque
encore est stockée mais pas annoncée ; la resynchronisation suivante la complète.

Il n'y a pas d'accusé de réception par opération ni de rejeu côté éditeur : un client
Yjs garde sa réplique et se resynchronise par vecteur d'état à la reconnexion, ce qui
est idempotent.

## Reprise

Au redémarrage, chaque document est reconstruit depuis son instantané puis les mises à
jour suivantes. Une mise à jour illisible est ignorée et signalée
(`stored_update_unreadable`) plutôt que de bloquer l'ouverture.

**Rattrapage du dossier.** Personne ne surveille le dossier quand le daemon est arrêté — et
lancé par un éditeur avec `--exit-with-parent`, il s'arrête avec lui. Avant d'ouvrir le réseau et le WebSocket, le daemon
compare donc chaque note à son fichier, à l'aide de l'empreinte du texte qu'il y avait vu en
dernier (`disk_state`) :

| Au démarrage | Conclusion | Effet |
| --- | --- | --- |
| Fichier identique à la note | rien ne s'est passé | — |
| Fichier différent, empreinte **inchangée** | c'est la note qui a avancé (édition d'un pair jamais projetée) | le fichier est réécrit |
| Fichier différent, empreinte **changée** | modifié pendant l'arrêt | importé dans la note, comme un diff minimal |
| Fichier absent, empreinte connue | supprimé pendant l'arrêt | la note est supprimée pour tous |
| Fichier absent, jamais vu sur disque | note reçue mais jamais écrite | le fichier est écrit |
| Fichier sans note | créé pendant l'arrêt — ou note déplacée, reconnue à son texte identique | note créée, ou renommée en gardant son identité |

Une base créée avant l'existence de ces empreintes n'en a aucune : à son premier démarrage,
un fichier absent est pris pour une suppression, ce qu'il était presque toujours.

**Compaction.** Toutes les 200 mises à jour, et à chaque ouverture, l'état complet du
document remplace son instantané et les mises à jour antérieures sont supprimées dans la
même transaction. La base ne grossit donc pas avec le nombre de frappes. L'historique
fin d'édition n'est pas conservé au-delà de ce que contient l'état Yrs (tombstones compris).

## Baux et fichiers ouverts

Une connexion d'éditeur ouverte sur un document est un bail. Tant qu'un bail existe, le
daemon n'écrit pas ce fichier et n'importe pas ses changements disque : l'éditeur est
responsable de son tampon. À la fermeture de la **dernière** connexion, le daemon écrit
aussitôt l'état convergé. Un bail ne survit pas au processus : il est la connexion même.

Les écritures de fichiers sont atomiques (fichier temporaire dans `.collab`, `fsync`,
renommage).

Voir le [format de stockage](../reference/storage.md), et [dépanner](../how-to/troubleshoot.md)
pour sauvegarder et restaurer.
