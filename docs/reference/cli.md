# Interface en Ligne de Commande (CLI)

La CLI `deepika-sync` fournit l'ensemble des commandes pour démarrer des sessions collaboratives, exposer le serveur LSP ou interroger l'état local du stockage.

---

## 1. Commandes Principales

| Commande | Description |
| :--- | :--- |
| `deepika-sync share <racine> [--ws-port <port>] [--relay] [--exit-with-parent]` | Ouvre une session collaborative P2P, démarre le transport Iroh QUIC et le serveur WebSocket `y-sync` (port par défaut `4444`). |
| `deepika-sync join <invitation> --root <racine> [--ws-port <port>] [--relay] [--exit-with-parent] [--preview \| --yes]` | Rejoint une session à partir d'un code d'invitation. **Affiche d'abord ce qui va changer dans le dossier et demande confirmation** (`o` pour accepter) ; tant que la réponse n'est pas oui, rien n'est écrit — ni `.collab`, ni journal. `--preview` imprime ce plan en JSON et s'arrête. `--yes` rejoint sans question : à réserver aux scripts et aux interfaces qui ont déjà montré le plan. Sans terminal et sans `--yes`, la commande refuse (`confirmation_required`). Refuse une racine déjà liée à une autre session. |
| `deepika-sync lsp [--root <racine>] [--ws-port <port>]` | Lance le serveur **LSP 3.17 sur stdio** pour Neovim, VS Code, Helix, Zed, etc. C'est un **client du daemon** : `share` ou `join` doit déjà servir cette racine sur ce port, sinon l'éditeur affiche une erreur explicite. |

---

## 2. Commandes d'Administration & d'Inspection

`status` et `documents` lisent l'index SQLite (`.collab/state.sqlite`) en lecture seule, **sans prendre le verrou de session** : elles fonctionnent pendant que le daemon tourne. `rename` et `delete` écrivent dans le CRDT et exigent le verrou : daemon arrêté uniquement. Pendant une session, renommez ou supprimez simplement le fichier sur disque — le daemon le répercute (erreur `session_already_running` sinon).

| Commande | Description |
| :--- | :--- |
| `deepika-sync status [--root <racine>]` | Affiche en JSON la racine, la capacité de la session et le nombre de documents vivants. La capacité est un secret : ne pas coller cette sortie en public. |
| `deepika-sync documents [--root <racine>]` | Liste tous les documents du workspace (UUID, chemin relatif, état de suppression). |
| `deepika-sync rename <UUID> <chemin> [--root <racine>]` | Hors session : renomme un document identifié par son UUID. Le fichier suit au prochain démarrage du daemon. |
| `deepika-sync delete <UUID> [--root <racine>]` | Hors session : marque un document comme supprimé dans le CRDT. |

---

## 3. Options Globales & Variables d'Environnement

- `--ws-port <port>` : port TCP du serveur WebSocket, en écoute sur `127.0.0.1` seulement (par défaut `4444`). Passer le même à `lsp`. `0` laisse le système choisir un port libre, publié dans `.collab/session.json` : utile pour plusieurs sessions sur un poste. Un port déjà pris fait échouer le lancement.
- `--exit-with-parent` : la session s'arrête proprement quand son **entrée standard se ferme**. Un programme qui lance le daemon garde un tube ouvert vers lui ; quand ce programme disparaît — fermé, planté ou tué —, le système ferme le tube et la session s'arrête avec lui, `session.json` retiré. Sans cette option, l'entrée standard est ignorée : un daemon lancé en arrière-plan ou depuis un terminal n'est pas concerné.
- Arrêt : `Ctrl-C` ou `SIGTERM`. Il n'y a pas de commande `leave`.
- `--relay` : utilise le relais public Iroh (preset `N0`) pour joindre un pair hors du réseau local, chiffré de bout en bout. Sans lui, seules les connexions directes et la découverte mDNS locale sont tentées. **Obligatoire** pour un invité distant et pour le client web.
- `RUST_LOG=deepika_sync=debug` : Active les logs détaillés sur la sortie standard d'erreur.
- `DEEPIKA_SYNC_LOG=deepika_sync=trace` : Configure le niveau de granularité des logs structurés dans `.collab/daemon.log`.
