import { useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

type Side = "top" | "bottom" | "left" | "right";

const POS: Record<Side, string> = {
  top: "bottom-full left-1/2 -translate-x-1/2 mb-1.5",
  bottom: "top-full left-1/2 -translate-x-1/2 mt-1.5",
  left: "right-full top-1/2 -translate-y-1/2 mr-1.5",
  right: "left-full top-1/2 -translate-y-1/2 ml-1.5",
};

// Lightweight hover tooltip — no portal, no dependency. Lets icon-only buttons
// stay compact while remaining discoverable.
export function Tooltip({
  label,
  children,
  side = "top",
}: {
  label: ReactNode;
  children: ReactNode;
  side?: Side;
}) {
  const [open, setOpen] = useState(false);
  return (
    <span
      className="relative inline-flex"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)}
      onBlur={() => setOpen(false)}
    >
      {children}
      {open && label && (
        <span
          role="tooltip"
          className={cn(
            "anim-fade pointer-events-none absolute z-tooltip whitespace-nowrap rounded-md border border-border bg-bg-panel px-2 py-1 text-[10px] font-medium text-text shadow-elev-2",
            POS[side]
          )}
        >
          {label}
        </span>
      )}
    </span>
  );
}
