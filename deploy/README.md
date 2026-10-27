# Deploy and recover the diary/publisher

Keep one writer per state path. Use a dedicated agent branch, run the README
checks, commit, then build with `DIARY_BUILD_REVISION=$(git rev-parse HEAD)` and
an external `CARGO_TARGET_DIR`. Record revision, binary SHA-256, compiler version,
host architecture/libc and observed results in the deployment receipt.

1. Stop `g8t-nextlevel-publisher.timer` and wait for any active oneshot to finish.
2. Back up the installed publisher, config and units, its entire state directory,
   and the diary binary/unit/state/status plus public `next-level` reports. Stop
   the diary briefly while taking the final consistent state backup. Preserve
   ownership and modes; record which services/timers were active.
3. Upload the new binary under a temporary name. Verify its digest and dynamic
   library compatibility on the diary host. Before installation, launch it on a
   spare loopback port with a **copy** of the live state and a private status file;
   check health/build revision, history and vote preservation, then stop it.
4. Stop the live diary, atomically rename the staged binary into place and start
   it. Verify `/health` identifies the expected build, `/api/diary` preserves
   history, `/api/capsela` labels restored votes, and every configured portal
   serves the expected revision. Check the installed binary digest too.
5. Install the publisher/config/units from this directory (the sample config
   names the existing fleet destinations; review it for the actual host):

   ```sh
   install -m 0755 scripts/publisher.py /usr/local/bin/g8t-nextlevel-publisher
   install -m 0600 deploy/publisher.json /etc/g8t-nextlevel-publisher.json
   install -m 0644 deploy/g8t-nextlevel-publisher.service /etc/systemd/system/
   install -m 0644 deploy/g8t-nextlevel-publisher.timer /etc/systemd/system/
   systemctl daemon-reload
   systemctl start g8t-nextlevel-publisher.service
   ```

   `gh` must authenticate as the configured service user and SSH must already
   have a trusted host key and scoped access. Verify the service exit code,
   `receipt.json`, remote observation and public report, plus the live API's
   recalculated ages. Run twice; an unchanged result must add no duplicate note.
   A stale/unverified source should produce an honest withheld receipt.
6. Restart the diary once, verify history/votes and build identity again, then
   `systemctl enable --now g8t-nextlevel-publisher.timer`. Confirm the next timer
   deadline. Publish the branch/PR and retain the deployment/memory receipt.

## Recovery

Pause the publisher timer first. Preserve failed/new snapshots and receipts in a
separate recovery directory. Restore the backed-up binary/unit and its matching
snapshot only while the diary is stopped; never overwrite newly accepted notes
without preserving them for reconciliation. Restart and verify the previous
revision and history. v2's event ledger is not understood by old releases.
Restoring the old publisher would restore its duplicate-reporting bug, so keep
its timer paused until the evidence-aware publisher passes validation. Corrupt
state must be recovered from a known-good backup, not deleted to reset the loop.
