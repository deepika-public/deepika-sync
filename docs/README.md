# Documentation deepika-sync

Le daemon possède la session, l'état des documents et la synchronisation ; les éditeurs
s'y connectent par LSP, par WebSocket y-sync, ou depuis un navigateur par le client web.
Le [README](../README.md) résume le projet ; cette page indexe le reste.

| | Pages |
| --- | --- |
| **Tutoriels** | [Première session à deux, en ligne de commande](tutorials/first-session.md) · [Observer une collision de chemin](tutorials/join-existing-folder.md) |
| **Guides** | [Installer, mettre à jour](how-to/install.md) · [Dépanner, sauvegarder, récupérer](how-to/troubleshoot.md) · [Tester, mesurer](how-to/test.md) · [Publier une release](how-to/release.md) |
| **Référence** | [CLI](reference/cli.md) · [Versions, compatibilité, plateformes](reference/compatibility.md) · [Rejoindre une session](reference/join-behavior.md) · [Protocoles éditeur](reference/editor-protocol.md) · [Stockage](reference/storage.md) · [Cas d'édition concurrente](reference/concurrency-cases.md) |
| **Explications** | [Architecture](explanation/architecture.md) · [Durabilité](explanation/durability.md) · [Modèle de sécurité](explanation/security-model.md) · [Journal des décisions](explanation/decisions.md) |
| **Client web** | [deepika-sync dans le navigateur](../web/README.md) |

Le plugin Obsidian a sa propre documentation, de même structure :
[dot-sync](https://gitlab.com/deepika-public/deepika-obsidian-toolbox/dot-sync/-/blob/main/README.md).
Il ne documente jamais le daemon, et cette documentation ne décrit jamais le plugin :
chacune renvoie à l'autre.

Règles de cette documentation : un sujet vit dans un seul fichier ; aucun numéro de version
ni lien figé en dehors de [compatibilité](reference/compatibility.md) ; le journal des
décisions ne se réécrit pas, il s'archive.
