# dsh persistent server lifecycle

Fleet talks to every dsh session through one local `dsh web` process. That
process is a machine-level authenticated service, not a child whose useful
lifetime ends with one desktop window.

## Registry contract

`~/.fleet/dsh-server.json` is the cross-process lease. A current record stores:

- the server PID and process start time;
- the last Fleet process that connected to it, for diagnostics and legacy
  migration only;
- the loopback port and dsh launch token;
- the dsh binary and workspace used to start it, so an adopted service can be
  restarted after a later health failure.

The file is atomically replaced under the existing cross-process lock and is
mode `0600` on Unix because the launch token authorizes every dsh RPC. Legacy
records without a token remain readable, but are never adoptable.

## Start and adoption

On the first dsh RPC in a Fleet process:

1. Lock and read the registry.
2. Remove records whose server PID/start-time no longer identifies a live
   process.
3. For each current-format record, exchange its launch token for a cookie and
   call the side-effect-free `settings/describe` endpoint.
4. Adopt the first healthy service and update its diagnostic owner to the
   current Fleet process.
5. Only when no record is adoptable, reclaim legacy unauthenticated or
   registry-invisible orphans and start a fresh `dsh web`.

Two Fleet clients may connect to the same authenticated service. Dropping a
client or closing the desktop stops only that client's event watcher; it does
not terminate the service or an in-flight turn.

## Failure and explicit stop

If an adopted service later fails its process or RPC health check, the caller
removes that exact PID/start-time record and starts a replacement. PID reuse
must never validate a stale record.

`dsh_source::shutdown()` remains an explicit machine-wide stop for tests,
upgrades, and user-requested teardown. Normal desktop exit, `fleet serve`
exit, and one-shot CLI completion must not call it. A current-format service
left after an application crash is authenticated and adoptable; a legacy
token-less orphan remains killable by `reap_orphans()`.

The signature sweep must exclude every live PID present in the registry. An
adopted service is normally reparented to PID 1, so parentage alone is no
longer evidence that it is abandoned.

## Acceptance invariant

Replacing and relaunching the Fleet desktop while a dsh turn is running keeps
the same dsh PID alive. The relaunched desktop authenticates to that PID,
rebuilds its event watcher, and continues observing the same turn without a
synthetic `reason=interrupted` boundary.
