# Bounded ETW DPC/ISR and disk inspection

## Current activation status

**Native activation is deliberately unavailable.** `collect` returns
`Unsupported / CleanupUnvalidated` before `StartTrace` or any permission change.
Portable aggregation, closed DTOs, the fixed TDH decoder and private-session cleanup
seams are implemented. Synthetic tests and successful compilation do not prove that a
real kernel logger can be safely cleaned up after the elevated helper is killed.

Microsoft documents that an ETW session can outlive its controller. The unelevated
desktop normally cannot stop it. Per-session `EventAccessControl` can grant control,
but those permissions are stored in the registry; the documented QUERY/STOP rights do
not prove that the parent can call `EventAccessRemove`. A StartTrace-success/ownership-
proof-write interval also exists during which `TerminateProcess` prevents helper
destructors from running. An identity alone does not close that interval.

Before activation, a controlled Windows acceptance must prove all of the following:

1. The session belongs to this approved job and creation really succeeded. An
   `AlreadyExists` result grants no authority over the existing session.
2. The parent can query and stop only this private session and remove any temporary
   GUID-specific permissions, without changing groups, providers or global privileges.
3. A fixed, privileged and bounded guardian is armed **before** StartTrace, so that
   forced termination of both controller and GUI cannot orphan the logger or rights.
4. Cleanup succeeds during normal completion, local cancellation, blocked consumer,
   watchdog `process::exit`, parent `TerminateProcess` and controller startup failure.

No environment setting or caller flag can bypass the current private preflight. No
ETW session, ACL or group was modified on the development PC by this implementation.

## Fixed collection design

The only durations are 30 and 60 seconds. The logger name derives from the local job
UUID; its GUID derives from the private job and nonce. Neither leaves the helper
protocol. It uses `EVENT_TRACE_SYSTEM_LOGGER_MODE` with its own private name/GUID and
real-time consumption, never `NT Kernel Logger`, `SystemTraceControlGuid` or a global
session. Only DPC, interrupt and disk-I/O flags are enabled; `NO_SYSCONFIG` suppresses
default configuration events. Image, file-name, file-I/O, stack and command-line
providers are absent.

Requested buffers are 64 KiB, 4 minimum and 32 maximum, without per-processor
buffering. Adjusted settings exceeding that bound stop the operation. At most 250,000
callbacks and six schema-cache entries are processed. Per-event payload and TDH
metadata allocations have fixed bounds; no raw event is retained or written to disk.

The decoder accepts only documented classic version-2 PerfInfo DPC types 66/68/69,
ISR type 67, and DiskIo read/write types 10/11. TDH must supply a reviewed scalar schema
from the matching provider. Only `InitialTime`, `TransferSize` and
`HighResResponseTime` can be requested. Routine addresses, file objects, IRPs, device
numbers, paths, process/thread IDs and content never enter the DTO or model projection.
Missing, changed or malformed schemas retain unsupported coverage.

## Timing and evidence

The consumer requests raw QPC timestamps. DPC/ISR duration uses the entry counter and
the event counter; disk response time uses the partition manager's documented QPC tick
duration. Conversion uses the measured QPC frequency with checked integer arithmetic,
rounded down to microseconds. Events beginning before the requested window, delivered
outside it, with reversed counters or unavailable timing are counted as excluded, not
used to fabricate latency. Events arriving out of order update first/last offsets by
minimum/maximum, never by callback order.

The four class aggregates describe accepted events only. Duration totals may overlap
across routines/CPUs and are not elapsed time or a CPU percentage. No matching events
means unknown duration/bytes, not zero activity. A known zero after microsecond rounding
is a quantized observation, not proof of an instantaneous routine. Driver attribution
is unavailable because image/address data is intentionally absent.

Final session `EventsLost` and `RealTimeBuffersLost` counters remain separate. Unknown
loss statistics remain absent with `LossCountersUnavailable`, including incomplete
STOP-property output. Consumer draining is bounded to two seconds; a drain
failure labels the accepted prefix incomplete. Cancellation, event/metadata limits,
schema/timing failures and losses never become completed coverage. Numeric results are
withheld if successful session cleanup is unconfirmed. Native accuracy remains
unvalidated until actual hardware acceptance.

## Integration seams

- `collect(&ElevationRequest, owner_sid, duration, cancel)` performs private preflight.
- `abort_owned_session()` stops only a successfully created active handle whose queried
  private GUID/mode match. Root must call it before every watchdog or hard-deadline
  `process::exit`, since destructors do not run there.
- `cleanup_for_request(&ElevationRequest, owner_sid)` is a local parent recovery seam.
  It remains unvalidated/inactive with the same preflight. Once certified, it queries
  only the secret-bound private name, checks GUID/mode and never enumerates/stops another
  session. A future ownership proof must gate the recovery path.

Fixtures cover QPC conversion, overflow, bounds, ordering, boundary exclusion, unknown
schemas, loss accounting, missing events, closed DTOs, private identity binding and the
no-effects preflight. They do not start a real logger or prove parent ETW permissions.

Primary references:

- [Microsoft: private SystemTraceProvider sessions](https://learn.microsoft.com/en-us/windows/win32/etw/configuring-and-starting-a-systemtraceprovider-session).
- [Microsoft: ETW properties, flags, buffers and losses](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/ns-evntrace-event_trace_properties).
- [Microsoft: DPC events and entry timestamps](https://learn.microsoft.com/en-us/windows/win32/etw/dpc).
- [Microsoft: ISR events](https://learn.microsoft.com/en-us/windows/win32/etw/isr).
- [Microsoft: disk I/O fields and QPC response-time units](https://learn.microsoft.com/en-us/windows/win32/etw/diskio-typegroup1).
- [Microsoft: ControlTrace](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nf-evntrace-controltracew).
- [Microsoft: per-session permissions](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/nf-evntcons-eventaccesscontrol),
  [permission removal from the registry](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/nf-evntcons-eventaccessremove).
