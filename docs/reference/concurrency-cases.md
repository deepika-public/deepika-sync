# Cas d'édition concurrente

Carte des situations où deux volontés touchent la même chose en même temps, avec le
test qui couvre chacune — ou l'aveu qu'aucun ne la couvre. Un cas ne figure ici que
s'il met en jeu un invariant qu'aucun autre cas ne met déjà en jeu.

## Les acteurs

| | Acteur | Détient un bail ? | Chemin de code |
| --- | --- | --- | --- |
| **E** | Éditeur connecté (LSP ou client Yjs) | oui, tant que sa socket est ouverte | `ws` → `Workspace::handle_y_msg` |
| **R** | Pair distant | non | `transport` → `Workspace::handle_y_msg` |
| **X** | Outil externe sur le disque (git, `sed`, un autre logiciel) | non | watcher → `Projector::handle_fs_event` |
| **D** | Le daemon lui-même (matérialisation) | — | `Projector::project_to_disk` |

Tous aboutissent à `Workspace::mutate`, l'unique chemin de mutation.

## Les garanties en jeu

- **G1** aucune contribution acceptée n'est perdue à la fusion
- **G2** identité stable : un UUID survit au contenu, au renommage, au déplacement
- **G3** convergence : mêmes textes et mêmes chemins sur tous les pairs
- **G4** ce qui est annoncé est déjà durable ; une mise à jour connue n'est ni réécrite ni rediffusée
- **G5** le tampon d'un éditeur connecté n'est jamais écrasé par le daemon
- **G6** confinement : rien hors de la racine, ni chemin caché, ni lien symbolique

## Contenu contre contenu

| Cas | Garanties | Couverture |
| --- | --- | --- |
| E × R, frappes simultanées | G1 G3 | propriété de Yrs ; `websocket_relays_updates_and_awareness_between_clients` |
| X × X, deux imports disque concurrents du même fichier | G1 G3 | `concurrent_disk_imports_merge_without_duplication` |
| E, positions avec accents et emoji (UTF-16) | G1 | `edits_use_utf16_offsets_with_accents_and_emoji`, `lsp_handshake_incremental_editing_and_remote_push` |
| R × E, modification distante d'un fichier loué | G5 | différée puis écrite à la fermeture : `releasing_the_last_lease_lets_the_projection_catch_up` |
| D × D, deux écritures rapprochées et un événement disque en retard | G1 G3 | `a_late_event_showing_our_previous_write_does_not_revert_the_note` ; `scripts/test-mesh.py` |
| X × R, modification extérieure dans les ~100 ms précédant une mise à jour distante | G1 | **non couvert** — écrasée avant d'être importée ; fenêtre connue |
| Mise à jour reçue deux fois (maillage avec cycle) | G3 G4 | `an_update_already_known_is_not_announced_again` ; `scripts/test-mesh.py` |

## Chemins : renommer, créer, supprimer

| Cas | Garanties | Couverture |
| --- | --- | --- |
| R × R, même chemin créé des deux côtés, textes différents | G1 G2 G3 | `same_path_created_on_two_peers_resolves_identically` |
| R × R, même chemin créé des deux côtés, textes identiques | G3 | `joining_with_an_identical_copy_keeps_a_single_note` |
| Chemin supprimé puis recréé | G2 | `a_deleted_path_can_be_created_again`, `legacy_unique_path_schema_is_migrated` |
| X, renommage disque d'un fichier suivi | G2 | `projection_disk_events_and_materialization` |
| R × R, deux renommages concurrents du même UUID | G3 | **non testé** — repose sur le registre last-writer-wins de `yrs::Map` |
| R × R, suppression contre édition | G3 | **non testé** — la suppression l'emporte, l'édition reste dans l'état |
| X × E, suppression ou écriture disque d'un fichier loué | G5 | **non testé** — ignorée tant que le bail dure, puis écrasée par l'état du CRDT à la fermeture, sans avertissement |
| Déplacement ou suppression d'un dossier entier | G2 | `moving_a_folder_moves_every_note_in_it_and_keeps_their_identity` ; harnais Obsidian du plugin |
| Renommage que le watcher n'a pas apparié (juste après une écriture du daemon) | G2 | `an_unpaired_rename_keeps_the_note_identity` |
| R × E, renommage distant d'une note ouverte | G2 G5 | différé jusqu'à la fin du bail, ou suivi par l'éditeur ; harnais Obsidian du plugin (`npm run test:desktop`) |
| R × E, doublon retiré alors qu'un éditeur le tient | G5 | marqueur `superseded_by` ; harnais Obsidian du plugin |

## Cycle de vie

| Cas | Garanties | Couverture |
| --- | --- | --- |
| Rechargement après arrêt | G4 | `sqlite_storage_lifecycle_and_reload` |
| Longue session : compaction puis rechargement | G4 | `long_sessions_are_compacted_and_reload_intact` |
| Deux connexions sur un document : bail jusqu'à la dernière fermeture | G5 | `releasing_the_last_lease_lets_the_projection_catch_up` |
| Second daemon sur la même racine ; inspection pendant la session | G4 | `index_follows_renames_and_is_readable_while_the_session_is_locked` |
| Consentement avant de rejoindre : le plan annoncé, et rien d'écrit avant l'accord | G5 | `tests/preview.rs` ; harnais Obsidian du plugin |
| Dossier modifié pendant l'arrêt du daemon : note supprimée, éditée, créée, déplacée | G4 | `what_happened_to_the_folder_while_stopped_is_caught_up_at_start` |
| Note ou édition d'un pair jamais écrite avant l'arrêt : écrite au redémarrage, pas prise pour une suppression | G4 | `a_note_that_never_reached_the_disk_is_written_not_deleted` |
| Hôte ou invité redémarré ; invité relancé sans invitation | G3 | `a_guest_calls_its_host_back_after_either_side_restarts` |
| Connexion réelle à deux et trois pairs | G3 | `scripts/test-two-peers.py`, `scripts/test-mesh.py` |
| Arrêt brutal (`SIGKILL`) en cours d'écriture | G4 | **non testé** — repose sur SQLite WAL `synchronous=FULL` |

## Sécurité et bords

| Cas | Garanties | Couverture |
| --- | --- | --- |
| Pair sans la capacité ; secret absent du `Hello` | G6 | `wrong_capability_receives_no_documents_and_learns_no_secret`, `right_capability_is_offered_the_documents` |
| Page web visant le WebSocket local | G6 | `websocket_refuses_browser_origins` |
| Liens symboliques sur l'état interne | G6 | `sqlite_internal_symlinks_are_rejected` |
| Chemin hostile reçu d'un pair (`..`, caché, absolu) | G6 | **non testé de bout en bout** — `Root::write` valide tout chemin avant d'écrire |
| Mise à jour stockée illisible | G4 | **non testé** — ignorée et signalée (`stored_update_unreadable`) |

## Ce qui n'est pas couvert, et pourquoi

- **Deux réseaux physiques, NAT réel, coupure d'alimentation** : hors de portée d'un
  test local. Voir [compatibilité](compatibility.md).
- **Course LSP** : `workspace/applyEdit` est asynchrone ; une frappe émise avant que
  l'éditeur ait appliqué une modification distante est positionnée sur son ancien
  texte. Le CRDT converge, la position peut être décalée. Limite du protocole, pas un
  invariant testable ici.
- Les lignes « non testé » ci-dessus sont la liste de travail, pas des garanties.
