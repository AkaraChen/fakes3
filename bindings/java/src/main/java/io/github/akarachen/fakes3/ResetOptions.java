package io.github.akarachen.fakes3;

/**
 * What {@link FakeS3#reset(ResetOptions)} clears besides the data. By default the request log and
 * the clock are reset and snapshots are kept.
 */
public final class ResetOptions {
  final boolean clock;
  final boolean requests;
  final boolean snapshots;

  private ResetOptions(boolean clock, boolean requests, boolean snapshots) {
    this.clock = clock;
    this.requests = requests;
    this.snapshots = snapshots;
  }

  /** The defaults: reset the clock and the request log, keep snapshots. */
  public static ResetOptions create() {
    return new ResetOptions(true, true, false);
  }

  /** Whether to put the clock back on the host clock; default true. */
  public ResetOptions clock(boolean reset) {
    return new ResetOptions(reset, requests, snapshots);
  }

  /** Whether to empty the request log; default true. */
  public ResetOptions requests(boolean clear) {
    return new ResetOptions(clock, clear, snapshots);
  }

  /** Whether to delete all snapshots; default false. */
  public ResetOptions snapshots(boolean delete) {
    return new ResetOptions(clock, requests, delete);
  }
}
