import type { SVGProps } from 'react';

// The Dissent mark from the icon sprite (ui/icons.svg, i-dissent-mark). Drawn in currentColor.
export function DissentMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="192.5 189 870 870" fill="currentColor" aria-hidden="true" focusable="false" {...props}>
      <path d="M235 230Q235 202 268 202H680C850 202 980 330 1012 490L780 588V535C778 450 710 374 632 374H450V750L238 850Q235 851 235 845Z M1017 547L780 638V665C778 760 710 828 630 828H410C385 828 362 838 350 858L244 1028Q238 1046 257 1046L340 1003Q345 1000 352 1000H705C900 1000 1020 860 1020 680Z" />
    </svg>
  );
}
