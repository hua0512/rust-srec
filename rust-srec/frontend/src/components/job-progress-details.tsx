import { Fragment } from 'react';

import { cn } from '@/lib/utils';

/**
 * A job's progress details, as `describeJobProgress` lists them, separated
 * by dots. Wraps only between details, so no value splits across lines; a
 * detail wider than the line, such as a long file name, is cut. Each dot
 * stays with the detail before it, so no line starts with a dot or holds
 * only one.
 *
 * Sets normal wrapping, overriding the balanced wrapping a tooltip passes
 * down, and tabular digits, so details move between lines as rarely as
 * possible while the values tick.
 */
export function JobProgressDetails({
  details,
  className,
}: {
  details: string[];
  className?: string;
}) {
  return (
    <div className={cn('text-wrap tabular-nums', className)}>
      {details.map((detail, index) => (
        <Fragment key={detail}>
          {index > 0 && ' '}
          <span className="whitespace-nowrap">
            <span className="inline-block max-w-full truncate align-bottom">
              {detail}
            </span>
            {index < details.length - 1 && ' ·'}
          </span>
        </Fragment>
      ))}
    </div>
  );
}
