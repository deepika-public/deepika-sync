#!/usr/bin/env python3
"""Read the daemon logs of one or more sessions and say what went wrong.

    python3 scripts/diagnose.py [RACINE ...]

A session writes `<racine>/.collab/daemon.log` (plus one rotated backup) whatever
started it, so this works the same for a daemon launched from a terminal and for one
launched by Obsidian. Exit code is 1 when something needs attention.
"""
import argparse, datetime, json, pathlib, sys

# What each code means, in the order a reader needs it: what happened, what to do.
CODES = {
    'path_collision':
        "Deux pairs ont créé le même chemin. Le document à l'identifiant le plus grand a été "
        "renommé `nom (conflit <id>).md`, à l'identique sur tous les pairs. Fusionner à la main si besoin.",
    'capability_mismatch':
        "Un pair s'est présenté sans la preuve de la bonne capacité. Invitation périmée, "
        "autre session, ou pair d'une version antérieure à 0.5.",
    'remote_message_rejected':
        "Un message de synchronisation reçu n'a pas pu être appliqué. Isolé : sans gravité, "
        "la resynchronisation suivante comble. Répété : pair incompatible ou état corrompu.",
    'stored_update_unreadable':
        "Une mise à jour stockée dans SQLite est illisible et a été ignorée au chargement. "
        "Vérifier le disque ; un pair à jour recomblera le document.",
    'watcher_error':
        "Le watcher disque a signalé une erreur. Des modifications faites hors éditeur ont pu "
        "être manquées : réenregistrer les fichiers concernés.",
    'mdns_discovery_unavailable':
        "La découverte locale n'a pas démarré (multicast indisponible). Sans gravité : "
        "les invitations fonctionnent toujours.",
    'session_already_running':
        "Un daemon tient déjà cette racine. `status` et `documents` restent utilisables ; "
        "pour renommer ou supprimer, agir sur le fichier lui-même.",
}

# Events that make the story of a session readable, in the log's own wording.
TIMELINE = {
    'session opened', 'peer_connected', 'peer_disconnected',
    'FS event creating new doc', 'FS event updating doc text',
}


def logs(root: pathlib.Path):
    private = root / '.collab'
    if not private.is_dir() and (root.name == '.collab' or root.suffix == '.log'):
        private = root.parent if root.suffix == '.log' else root
    found = sorted(private.glob('daemon.log*')) if private.is_dir() else []
    if root.is_file():
        found = [root]
    return found


def read(paths):
    for path in paths:
        for number, line in enumerate(path.read_text(errors='replace').splitlines(), 1):
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            entry['_file'] = path
            entry['_line'] = number
            yield entry


def when(entry):
    return entry.get('timestamp', '')


def local(stamp):
    try:
        return datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00')) \
            .astimezone().strftime('%H:%M:%S')
    except ValueError:
        return stamp[:19]


def code_of(entry):
    fields = entry.get('fields', {})
    if fields.get('code'):
        return fields['code']
    # A conflict carries its code inside the error text, as `code:detail`.
    for key in ('error', 'message'):
        value = str(fields.get(key, ''))
        head = value.split(':', 1)[0].strip()
        if head in CODES:
            return head
    return None


def describe(entry):
    fields = {k: v for k, v in entry.get('fields', {}).items() if k != 'message'}
    detail = ' '.join(f'{k}={v}' for k, v in fields.items())
    return f"{fields.get('message', entry.get('fields', {}).get('message', ''))} {detail}".strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('roots', nargs='*', default=['.'],
                        help="racines de session, répertoires .collab ou fichiers de log")
    parser.add_argument('--timeline', action='store_true', help="afficher le déroulé complet")
    parser.add_argument('--bundle', metavar='FICHIER',
                        help="écrire le rapport dans un fichier à transmettre")
    args = parser.parse_args()

    paths = []
    for name in args.roots:
        found = logs(pathlib.Path(name).expanduser())
        if not found:
            print(f"Aucun journal sous {name} (cherché .collab/daemon.log*)", file=sys.stderr)
        paths += found
    if not paths:
        return 2

    entries = sorted(read(paths), key=when)
    out = []
    def say(text=''):
        out.append(text)

    say(f"Journaux : {', '.join(str(p) for p in paths)}")
    if not entries:
        say("Aucune entrée. Le daemon n'a peut-être jamais démarré ici.")
        print('\n'.join(out))
        return 2

    say(f"Période   : {local(when(entries[0]))} → {local(when(entries[-1]))}"
        f"  ({len(entries)} entrées)")
    say()

    sessions = [e for e in entries if e.get('fields', {}).get('event') == 'session opened']
    say(f"Sessions ouvertes : {len(sessions)}")
    for e in sessions:
        f = e['fields']
        say(f"  {local(when(e))}  version={f.get('version')} documents={f.get('documents')}")
    say()

    problems = [e for e in entries if e.get('level') in ('WARN', 'ERROR')]
    if not problems:
        say("Aucun avertissement ni erreur.")
    else:
        grouped = {}
        for e in problems:
            grouped.setdefault(code_of(e) or 'sans code', []).append(e)
        say(f"Points d'attention : {len(problems)} entrées, {len(grouped)} types")
        say()
        for code, group in sorted(grouped.items(), key=lambda kv: -len(kv[1])):
            first, last = local(when(group[0])), local(when(group[-1]))
            span = first if first == last else f"{first} → {last}"
            say(f"■ {code}  ×{len(group)}  ({span})")
            if code in CODES:
                say(f"   {CODES[code]}")
            seen = []
            for e in group:
                line = describe(e)
                if line not in seen:
                    seen.append(line)
            for line in seen[:5]:
                say(f"   · {line}")
            if len(seen) > 5:
                say(f"   · … {len(seen) - 5} autres formulations")
            say()

    if args.timeline:
        say("Déroulé :")
        for e in entries:
            fields = e.get('fields', {})
            message = fields.get('message') or fields.get('event', '')
            if e.get('level') in ('WARN', 'ERROR') or message in TIMELINE:
                mark = {'WARN': '!', 'ERROR': 'X'}.get(e.get('level'), ' ')
                say(f"  {mark} {local(when(e))}  {describe(e)}")
        say()

    errors = [e for e in problems if e.get('level') == 'ERROR']
    if errors:
        say(f"VERDICT : {len(errors)} erreur(s) — à traiter avant publication.")
    elif problems:
        say("VERDICT : aucun échec, mais des conflits explicites ont été signalés. "
            "Vérifier qu'ils correspondent à ce que vous avez provoqué pendant le test.")
    else:
        say("VERDICT : rien à signaler.")

    report = '\n'.join(out)
    print(report)
    if args.bundle:
        pathlib.Path(args.bundle).write_text(report + '\n')
        print(f"\nRapport écrit dans {args.bundle}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
