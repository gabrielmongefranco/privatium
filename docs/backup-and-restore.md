<!--
Project:  Privatium™
File:     docs/backup-and-restore.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-09-03
Summary:  The backup and restore procedure, written to be usable by a non-technical owner.
-->

# Backup and Restore

## The short version

**Copy the `data` folder. That's the backup.**

**Copy it back. That's the restore.**

Everything else in this document is elaboration.

## 1. What to back up

| Folder | Back it up? | Why |
|---|---|---|
| `data/` | **Yes** | Every event. This is your information. |
| `identity/` | Yes, separately and privately | Your node's key. Losing it means re-pairing devices; leaking it is worse than losing it. |
| `apps/` | Optional | Re-downloadable. Back it up if you customized one. |
| `config.toml` | Optional | Two minutes to recreate. |
| `local/` | **No** | Node-specific. Copying it to another machine causes confusion, not recovery. |
| `cache/` | **No** | Rebuilt on demand from `data/`. |

Everything in `data/` is a text file. You can open one in Notepad and read your own
information. This is on purpose. If a backup format needs special software to inspect, it
is not a backup, it is a hostage situation.

## 2. Choosing a method

| Method | Setup | Good for |
|---|---|---|
| **Syncthing** | Install on two machines, point both at `data/` | Continuous, automatic, no cloud, no account |
| **Dropbox / OneDrive / Drive** | Put `data/` inside the synced folder | People who already have it running |
| **USB stick, monthly** | Drag and drop | People who want something they can hold |
| **rsync / cron** | One line | People reading this section for fun |

**All of them work.** No configuration in Privatium is required for any of them. This is
the direct consequence of the single-writer log rule: two devices never edit the same file,
so a file syncer can never produce a conflict. If your syncer has a "conflicted copy"
feature, it will never fire on `data/`.

**Do not** point a sync tool at a live database file from any other application; that is
where the folklore about sync corrupting data comes from, and it does not apply here
because Privatium keeps nothing important in a database file.

## 3. Restore

### On a new machine

1. Install Privatium.
2. Start it once, then stop it. This creates the folder structure.
3. Copy your backed-up `data/` folder over the empty one.
4. Copy `identity/` back if you have it. If you do not, that is survivable — you get a new
   node identity and re-pair your devices. **Your information is not in `identity/`.**
5. Start Privatium.

It rebuilds everything — database, snapshots, views — from the text files. Depending on
history size this takes seconds.

`privatium restore --from <the backup>` does steps 3 and 5 in one go and tells you which
of the three tiers below it read from. Add `--dry-run` to see what it would copy first.
It never overwrites a log file it cannot reconcile with the one already there
(`spec/cli.md §7`).

### From a partial backup

The system reads in three tiers, and tells you which one it used:

| You have | Result |
|---|---|
| Everything | Normal. Fast. |
| Logs but no snapshots | Full rebuild. Slower once, then normal. Nothing lost. |
| Snapshots but damaged SQLite files | Falls back to the CSV copies automatically. |
| Only some log files | Those devices' history is restored; the others are missing. Partial, honest, no crash. |
| One log file, opened in a text editor | You can still read your data with your eyes. |

If the node starts in tier 3 it says so on the home screen and writes a `restore.tier3`
alert. That message means "I rebuilt from scratch," not "something is broken."

## 4. Verify your backup — do this once

A backup you have never restored is a rumour.

1. Copy `data/` to a second computer or a second folder.
2. Start Privatium pointed at the copy (`--data-dir`).
3. Confirm your information is there.
4. Delete the test copy.

Ten minutes, once. Put a reminder in whatever you use for reminders.

## 5. Snapshots

Snapshots are written weekly by default into `data/<app>/snap/`. Each contains:

- `*.sqlite` — one database per table, opens in DB Browser for SQLite, the `sqlite3` shell,
  Python, R, and anything else that reads SQLite, which is everything
- `*.csv` — one file per table, opens in Excel or Numbers
- `schema.sql` — the exact column types, so the CSVs restore correctly
- `MANIFEST.json` — row counts and checksums

Snapshots are a convenience, not a backup. They speed up startup and give you clean exports
for analysis. Deleting all of them loses nothing.

Default retention is one year. The oldest snapshot is never deleted, whatever the setting
says.

## 6. Taking your data elsewhere

You are not locked in, and here is the proof:

```bash
# Read your entire history with no Privatium installed
cat data/hello/log/*.jsonl | jq .

# Or a snapshot, with the sqlite3 shell that ships with most operating systems
sqlite3 data/hello/snap/<snapshot-id>/profile.sqlite 'SELECT * FROM profile'

# Or with nothing at all
less data/hello/log/*.jsonl
```

The CSV files open in Excel by double-clicking.

## 7. What backup does not protect against

- **Someone stealing the backup.** It is plain text. Encrypt the destination — Syncthing
  over the network is already encrypted in transit; a USB stick in a drawer is not.
- **Deleting data on purpose and syncing it.** Sync propagates your deletions. Keep one
  periodic copy that is *not* live-synced.
- **Losing `identity/` and needing the same node ID.** Nothing recovers that. Store it
  with your passwords, not with your data.

---

Copyright © 2026 Gabriel Mongefranco
