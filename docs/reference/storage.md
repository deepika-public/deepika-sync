# Stockage local

Tout l'état privé réside sous `<racine>/.collab/` (permissions mode `0700`).

| Élément | Rôle |
| --- | --- |
| `state.sqlite` | Base SQLite embarquée : documents, mises à jour binaires Yrs, instantanés et configuration |
| `state.sqlite-wal`, `state.sqlite-shm` | Fichiers gérés par SQLite (WAL) |
| `lock` | Verrou consultatif exclusif : un seul daemon par racine (`session_already_running` sinon) |
| `daemon.log`, `daemon.log.1` | Journal JSON structuré, borné à 8 Mio, plus une sauvegarde tournante |
| `session.json` | Présent tant que le daemon tourne (mode `0600`) : `version`, `pid`, `root`, `ws_port`, `invite`. C'est par lui que le plugin Obsidian et tout autre outil trouvent la session ; supprimé à l'arrêt propre. Un fichier dont le `pid` est mort est un reste d'arrêt brutal |
| `tmp-<uuid>` | Fichier temporaire d'une écriture atomique en cours |

SQLite utilise `journal_mode=WAL` et `synchronous=FULL` pour garantir une résistance maximale aux coupures impromptues.

### Schéma des Tables

1. **`documents`** (`id`, `path`, `deleted`) : simple **index** du workspace. La source de vérité du chemin et de la suppression est la map `metadata` du document Yrs ; l'index est réécrit dans la même transaction que chaque mise à jour. `path` n'est pas unique : un chemin supprimé peut être recréé (les bases antérieures à 0.5 sont migrées à l'ouverture).
2. **`doc_updates`** (`document_id`, `seq`, `data`) : flux des mises à jour binaires Yrs v1.
3. **`snapshots`** (`document_id`, `seq`, `data`) : un instantané par document. Toutes les 200 mises à jour, et à chaque ouverture, l'état complet est écrit ici et les `doc_updates` antérieures sont supprimées dans la même transaction.
4. **`config`** (`key`, `value`) : `capability`, `secret` (clé privée Iroh), `bind_port`, `initialized`, `disk_tracking` (posé après la première réconciliation au démarrage), `peers` (adresses des pairs déjà appelés, dont l'hôte de l'invitation : c'est ce qui permet de les rappeler après un redémarrage).
5. **`disk_state`** (`document_id`, `hash`) : empreinte BLAKE3 du texte **vu pour la dernière fois dans le fichier** de chaque note, que le daemon l'ait écrit ou lu. C'est ce qui permet, au démarrage, de distinguer un fichier modifié ou supprimé pendant l'arrêt d'une note qui n'avait simplement pas encore atteint le disque.

Les écritures sur disque des fichiers Markdown projetés sont découplées et différées lorsqu'un éditeur détient un bail actif (`is_leased`), garantissant qu'aucune frappe en cours n'est perturbée.

Voir [durabilité](../explanation/durability.md) et [sécurité](../explanation/security-model.md).
