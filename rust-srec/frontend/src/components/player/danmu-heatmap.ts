const BINS = 120;
const HEIGHT_PX = 24;
const SVG_NS = 'http://www.w3.org/2000/svg';

/**
 * Comment counts in `bins` equal slices of `duration`, smoothed over each
 * slice's neighbours and scaled to 0..1. Comments past `duration` (a
 * recording cut short) land in the last slice.
 */
function danmuDensity(
  times: Float64Array,
  duration: number,
  bins = BINS,
): Float64Array {
  const counts = new Float64Array(bins);
  if (!(duration > 0)) return counts;
  for (const time of times) {
    counts[Math.min(bins - 1, Math.floor((time / duration) * bins))] += 1;
  }

  const smoothed = new Float64Array(bins);
  let max = 0;
  for (let bin = 0; bin < bins; bin++) {
    const previous = counts[Math.max(0, bin - 1)];
    const next = counts[Math.min(bins - 1, bin + 1)];
    smoothed[bin] = (previous + 2 * counts[bin] + next) / 4;
    max = Math.max(max, smoothed[bin]);
  }
  if (max > 0) for (let bin = 0; bin < bins; bin++) smoothed[bin] /= max;
  return smoothed;
}

/** A closed area path over a `values.length` x 1 viewBox, peaks at the top. */
function densityPath(values: Float64Array): string {
  let path = 'M0 1';
  for (let bin = 0; bin < values.length; bin++) {
    const y = (1 - values[bin]).toFixed(3);
    path += ` L${bin} ${y} L${bin + 1} ${y}`;
  }
  return `${path} L${values.length} 1 Z`;
}

/**
 * Draws a recording's danmu density as a shaded strip just above the player's
 * progress bar, so busy moments are visible before seeking to them.
 */
export class DanmuHeatmap {
  private readonly svg: SVGSVGElement;
  private readonly path: SVGPathElement;
  private readonly video: HTMLVideoElement;
  private readonly times: Float64Array;
  private drawnFor = -1;

  constructor(
    progressBar: HTMLElement,
    video: HTMLVideoElement,
    times: Float64Array,
  ) {
    this.video = video;
    this.times = times;
    this.svg = document.createElementNS(SVG_NS, 'svg');
    this.svg.setAttribute('preserveAspectRatio', 'none');
    this.svg.setAttribute('aria-hidden', 'true');
    Object.assign(this.svg.style, {
      position: 'absolute',
      left: '0',
      bottom: '100%',
      width: '100%',
      height: `${HEIGHT_PX}px`,
      marginBottom: '4px',
      pointerEvents: 'none',
    });
    this.path = document.createElementNS(SVG_NS, 'path');
    this.path.setAttribute('fill', 'rgba(255, 255, 255, 0.22)');
    this.svg.append(this.path);
    progressBar.append(this.svg);

    video.addEventListener('durationchange', this.draw);
    this.draw();
  }

  setVisible(visible: boolean): void {
    this.svg.style.display = visible ? '' : 'none';
  }

  destroy(): void {
    this.video.removeEventListener('durationchange', this.draw);
    this.svg.remove();
  }

  private readonly draw = (): void => {
    // Streamed recordings may report no duration; the last comment then
    // stands in for the end.
    const duration = Number.isFinite(this.video.duration)
      ? this.video.duration
      : (this.times.at(-1) ?? 0);
    if (duration === this.drawnFor) return;
    this.drawnFor = duration;
    const values = danmuDensity(this.times, duration);
    this.svg.setAttribute('viewBox', `0 0 ${values.length} 1`);
    this.path.setAttribute('d', densityPath(values));
  };
}
