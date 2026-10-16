# Display transition investigation

## Requirement and baseline

Fully disconnect the built-in panel without briefly blanking the external
monitor. Preserve automatic unplug recovery, one-way emergency restore, and
session-only startup. The user reports that seamless transitions occurred
previously, but no reproducible full-disconnect method has been established.

Recovery was physically confirmed after `6d94406`. Runtime code at `40322fb`
retains that recovery and reverts unsuccessful fade experiments. The research
below used temporary helpers; none of these changes were installed in the app.

## Observations on 2026-10-16

Test machine: M2 MacBook Air, macOS 26.6.2, ASUS XG32UCWMG, 3840x2160 at
144 Hz with a 1920x1080 desktop. Original arrangement: built-in mirrors ASUS,
ASUS is main. The user described the ASUS interruption as a black picture
without its input/resolution banner. This does not establish whether the link
reset or the compositor supplied black frames.

| Experiment | Result |
| --- | --- |
| `CGConfigureDisplayFadeEffect` with zero durations | Configuration accepted, commit rejected with error 1006. Reverted. |
| Separate fade reservation | ASUS still blanked. A later direct probe found `CGAcquireDisplayFadeReservation` itself returns 1006 with token zero on this machine. This was not a successful reservation. |
| Explicitly configure ASUS's existing mode in the same disable transaction | Accepted; user confirmed ASUS still blanked. |
| Unmirror into an extended desktop, then run original full disconnect | User confirmed ASUS still blanked. Mirroring is not required to reproduce it. |
| Disable with `.forAppOnly` instead of `.forSession` | User confirmed ASUS still blanked. Supervisor also observed an absent-external snapshot and a failed restore verification before recovering/re-disabling. Heavy load confounds interpretation. Do not install. |
| Native built-in backlight set to zero | User confirmed ASUS stayed visible. Explicitly rejected as a solution because the panel must fully disconnect for HDR. Original brightness restored. |

Timed full-disconnect helpers retained the production recovery implementation.
All completed with built-in restoration. The original mirrored arrangement was
restored after the extended-desktop experiments.

The external CG mode remained unchanged in software observations. Before/after
external IOMobileFramebuffer dictionaries were identical; sampling was too slow
to exclude a transient hardware change. Neither observation proves absence of
a visual interruption or preservation of HDR throughout a transition.

The installed public `CGCompleteDisplayConfiguration` implementation branches
directly to `SLSCompleteDisplayConfigurationWithOption`. Merely switching the
function name with the same scope does not select a different implementation.
Permanent scope was not tested because reboot must remain a recovery path.

## Remaining lead and limits

Later investigation found a separate, confirmed idle-transition bug: probe
timeouts were encoded as an absent external display in both watchdogs. Under
load, the supervisor restored the laptop, then disabled it again after a
successful probe; the independent watchdog could also restart the worker after
uncertain recovery. Logs showed these repeated transitions without a physical
unplug. Unknown topology now has its own state and suspends automatic switching
after recovery until explicitly enabled again. This fixes that feedback loop;
it does not establish a seamless intentional full-disconnect transition or
explain blackouts observed with automation fully stopped.

System load was unusually high (one observed one-minute load average exceeded
240), and even short subprocesses sometimes took seconds. A bounded local
WindowServer log scan found repeated 250 ms acknowledgment timeouts for
`kCGSDisplayDidReconfigure2` across many clients during the experiments.
This is a reason to compare transitions under normal load; it is not proof
that load causes the physical blackout. Do not weaken recovery checks to mask
probe timeouts. Do not terminate unrelated apps or restart WindowServer.

A useful next comparison is the unchanged full-disconnect path under normal
system load, recording visual outcome, external mode/HDR state, and transition
timing. A clean-session comparison requires coordinating a restart with the
user. No restart was performed during this investigation.

## Primary references checked

- [BlackLid](https://github.com/Elmosius/BlackLid) uses the app-scoped private
  commit path. Its existence did not predict a seamless transition here.
- [Clamshell reverse engineering](https://alinpanaitiu.com/blog/turn-off-macbook-display-clamshell/)
  explores privileged display-power control but does not provide a completed
  alternative implementation. Do not cite it as proof of a working method.
- [blackoutd's investigation](https://github.com/toobuntu/blackoutd/blob/main/docs/decisions/0010-cursor-on-black-detection-and-recovery.md)
  concerns a different, persistent post-wake black-screen failure. It reinforces
  that healthy software state alone cannot establish visible output; its global
  display-sleep workaround does not satisfy this project's seamless-transition goal.

Keep raw logs, local paths, device identifiers, and temporary binaries out of
the public repository. Record eyewitness results separately from hypotheses.
