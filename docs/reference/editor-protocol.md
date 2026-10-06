# Protocoles éditeur : LSP et WebSocket

Aucun protocole maison : un éditeur parle au daemon par l'un de deux standards, et un
éditeur ne parle jamais aux pairs — le transport appartient au daemon.

1. **LSP 3.17** sur stdio, pour les éditeurs de code (Neovim, VS Code, Helix, Zed, Emacs).
2. **WebSocket y-sync et y-awareness**, pour Obsidian, le client web et tout client Yjs.

Un éditeur ne gère aucune fusion : Yrs identifie chaque opération par `(client, horloge)`
et applique les mises à jour de façon commutative et idempotente, donc deux frappes
concurrentes convergent sans branche ni index côté éditeur.

## 1. Serveur LSP (stdio)

Le serveur LSP se lance avec :
```bash
deepika-sync lsp [--root <racine>] [--ws-port 4444]
```

Le serveur LSP est un **client du daemon** : pour chaque tampon ouvert il tient une réplique Yrs locale, synchronisée par le WebSocket décrit plus bas. Il n'ouvre ni SQLite ni le verrou de session ; `share` ou `join` doit donc tourner sur la même racine. Plusieurs éditeurs peuvent se connecter au même daemon.

Il communique via **stdio** selon la spécification JSON-RPC 2.0 LSP 3.17 (`Content-Length: <n>\r\n\r\n<json>`).

### Cycles de Vie et Méthodes Gérées

| Méthode LSP | Sens | Rôle |
| :--- | :---: | :--- |
| `initialize` | Client $\rightarrow$ Serveur | Négocie les capacités (`textDocumentSync: Incremental`). |
| `textDocument/didOpen` | Client $\rightarrow$ Serveur | Ouvre la connexion WebSocket du document (= bail), attend la synchronisation initiale, puis aligne le tampon de l'éditeur sur le CRDT par `workspace/applyEdit` s'ils diffèrent. Daemon injoignable : `window/showMessage` d'erreur. |
| `textDocument/didChange` | Client $\rightarrow$ Serveur | Deltas incrémentaux, positions en **unités UTF-16** (accents et emoji compris). Appliqués à la réplique puis envoyés au daemon. |
| `textDocument/didClose` | Client $\rightarrow$ Serveur | Ferme la connexion : le daemon libère le bail et écrit aussitôt l'état convergé sur disque. |
| `workspace/applyEdit` | Serveur $\rightarrow$ Client | Modifications distantes : le serveur calcule le diff minimal entre le texte que l'éditeur connaît et sa réplique, et l'envoie. |

**Limite du mode LSP.** `workspace/applyEdit` est asynchrone : une frappe émise par
l'éditeur avant qu'il ait appliqué une modification distante est positionnée sur son
ancien texte. Le CRDT converge, mais l'insertion peut atterrir quelques caractères à
côté. Les clients Yjs n'ont pas cette limite.

### Configurations

Neovim, dans `init.lua` avec `nvim-lspconfig` :

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')
if not configs.deepika_sync then
  configs.deepika_sync = {
    default_config = {
      cmd = { 'deepika-sync', 'lsp' },
      filetypes = { 'markdown' },
      root_dir = lspconfig.util.root_pattern('.collab', '.git'),
    },
  }
end
lspconfig.deepika_sync.setup{}
```

Helix, dans `~/.config/helix/languages.toml` :

```toml
[[language]]
name = "markdown"
language-servers = [ "deepika-sync" ]

[language-server.deepika-sync]
command = "deepika-sync"
args = ["lsp"]
```

VS Code : un client LSP générique lançant `deepika-sync lsp` sur stdio.

## 2. Serveur WebSocket (y-sync et y-awareness)

Le serveur WebSocket est lancé automatiquement avec les commandes de partage :
```bash
deepika-sync share <racine> [--ws-port 4444]
deepika-sync join <invite> --root <racine> [--ws-port 4444]
```

### Routage Multi-Documents par URL

Chaque document du coffre / workspace est adressable directement via son chemin relatif dans l'URL HTTP lors du handshake WebSocket :
```text
ws://127.0.0.1:4444/notes/architecture.md
```
Le chemin est décodé (`%20`, UTF-8). Il peut aussi être l'UUID du document. Un chemin inconnu crée la note (depuis le fichier s'il existe). Si le chemin est omis, le premier document vivant est servi.

### Contrôle de l'origine

Le serveur n'écoute que sur `127.0.0.1`, mais n'importe quelle page web peut viser cette adresse. Le handshake refuse donc (HTTP 403) tout en-tête `Origin` en `http(s)://`. Sont acceptés : l'absence d'`Origin` (clients natifs, `deepika-sync lsp`) et les schémas d'application (`app://obsidian.md`, `file://`).

### Format des Messages Binaires (`y-sync`)

Les trames sont celles du protocole [y-protocols](https://github.com/yjs/y-protocols) de Yjs, décodées par `yrs::sync` :

- **`SyncStep1(StateVector)`** : Émis dès la connexion pour annoncer le vecteur d'état actuel.
- **`SyncStep2(Update)`** : Réponse contenant exactement les deltas manquants pour amener le pair à jour.
- **`Update(Update)`** : Émis en continu lors de chaque frappe ou insertion de texte.
- **`Awareness(Payload)`** : curseurs, sélections et présence. Le daemon ne les interprète pas : il les **relaie tels quels** aux autres connexions du même document, locales et distantes.
- **`AwarenessQuery`** : « qui est là ? ». L'awareness ne voyage que par changements ; le daemon garde donc en mémoire, par document, le dernier état de chaque éditeur (jamais persisté, oublié après 30 s de silence ou à son départ). Il l'envoie de lui-même à toute connexion qui arrive, et en réponse à cette requête.

Un message n'est jamais renvoyé à la connexion qui l'a émis, et une mise à jour déjà connue n'est pas rediffusée.

### Métadonnées du document

Outre le texte (`content`), chaque `Y.Doc` porte une map `metadata` qu'un client peut observer :

| Clé | Sens |
| :--- | :--- |
| `path` | Chemin de la note dans la racine. Il change quand la note est renommée, ici ou chez un pair ; un éditeur qui tient la note ouverte devrait renommer son fichier en conséquence. |
| `deleted` | `true` : la note a été supprimée. |
| `superseded_by` | Présent avec `deleted` quand deux pairs avaient la même note (même chemin, même texte) : la session n'a gardé que l'UUID indiqué. Rien n'a été supprimé ; le client se reconnecte au même chemin. |

Un client trouve le port et l'invitation de la session dans `<racine>/.collab/session.json`
([stockage](storage.md)). Aucune couche d'adaptation n'est nécessaire avec `yjs`,
`y-websocket` et `y-codemirror.next` : c'est ainsi que sont faits le plugin Obsidian et le
[client web](../../web/README.md).

## Baux : quand le disque est écrit

Tant qu'au moins une connexion (LSP ou WebSocket) tient un document, le daemon ne projette
pas ce document sur disque et n'importe pas les changements de son fichier, pour ne
jamais écraser un tampon actif. À la fermeture de la **dernière** connexion, l'état
convergé est écrit. Le détail est dans [durabilité](../explanation/durability.md).
