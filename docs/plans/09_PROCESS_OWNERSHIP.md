# Plan 09 — Crash-proof process ownership

**Priority:** P1  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)

## Objective

Anything ReflexDesk owns must terminate on explicit quit and, where the OS
allows it, when the parent crashes. User-owned apps must remain untouched.

## Ownership classes

- INTERNAL: STT/reflex/planner helpers; always terminate.
- OWNED_SESSION: agent/browser automation instance launched by ReflexDesk.
- ATTACHED: pre-existing external service/session; detach only.
- USER_APP: Spotify/Chrome/VS Code opened for user; never auto-kill.

## Platform strategy

Windows:
- Job Objects
- KILL_ON_JOB_CLOSE
- assign full owned process tree

Linux/macOS:
- dedicated process groups/session IDs
- SIGTERM group
- bounded grace period
- SIGKILL group

## Work packages

1. Replace bare Child map with ProcessHandle abstraction.
2. Track pid/process-group/job identity and ownership type.
3. Graceful shutdown callbacks.
4. crash/force-kill tests.
5. prevent PID-reuse mistakes.
6. child-tree diagnostics.

## Definition of done

Tests prove owned grandchildren do not survive normal quit or forced parent
termination, while USER_APP/ATTACHED processes are preserved.

## Acceptance notes

- Windows owned processes are created suspended, assigned to a
  `KILL_ON_JOB_CLOSE` Job Object, then resumed. Piped and non-piped spawn paths
  preserve the zero-execution-before-assignment invariant.
- Linux owned processes use dedicated process groups plus `PR_SET_PDEATHSIG`.
- macOS owned processes use dedicated process groups plus a guardian process
  watching a parent-liveness pipe; EOF terminates the owned group.
- Diagnostics record OS-derived process start identity where the platform
  exposes it and the actual process-group/job identity instead of synthetic IDs.
- The old unsupervised `track` path and direct user-app launcher were removed;
  every app launch now goes through the supervisor ownership classes.
- Acceptance remains pending until platform CI proves grandchildren terminate
  after normal quit and forced parent death while `USER_APP`/`ATTACHED`
  processes survive.
