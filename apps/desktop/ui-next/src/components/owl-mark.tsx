import type { SVGProps } from 'react';

// The Dissent owl, drawn in currentColor so it takes the text colour it sits in. The same outline as assets/icon/app_icon.svg,
// with the eyes and beak cut out (even-odd), so the background shows through them.
export function OwlMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="140 242 973 784" fill="currentColor" aria-hidden="true" focusable="false" {...props}>
      <path fillRule="evenodd" d="M626 392C560 340 470 305 380 305C300 305 255 280 230 242C218 300 225 360 248 395L203 393C212 420 225 448 240 468C180 530 140 620 140 697L170 688C156 720 156 760 158 780C160 925 380 1030 626 1026C872 1026 1092 925 1095 780C1097 760 1097 720 1085 688L1113 697C1113 620 1072 530 1014 468C1027 448 1040 420 1050 393L1005 395C1028 360 1035 300 1025 243C1000 280 950 305 870 305C780 305 690 340 626 392ZM314 657A116 116 0 1 0 546 657A116 116 0 1 0 314 657ZM708 657A116 116 0 1 0 940 657A116 116 0 1 0 708 657ZM552 754L698 754L624 878Z" />
    </svg>
  );
}
