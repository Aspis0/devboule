// Toast — the single transient confirmation chip (prototype's Toast). Renders the
// `.toast` (CheckCircle + message) when `msg` is set, and auto-dismisses after
// `duration` ms by calling `onDismiss`. The timer is cleared on unmount AND whenever
// the message changes (a new toast restarts the countdown; no overlap, no leak).

import { useEffect } from "react";
import { CheckCircle } from "lucide-react";

export interface ToastProps {
  msg: string | null;
  onDismiss: () => void;
  /** Auto-dismiss delay in ms (prototype uses 2400). */
  duration?: number;
}

export function Toast({ msg, onDismiss, duration = 2400 }: ToastProps) {
  useEffect(() => {
    if (!msg) return;
    const t = setTimeout(onDismiss, duration);
    return () => clearTimeout(t);
  }, [msg, duration, onDismiss]);

  if (!msg) return null;
  return (
    <div className="toast" role="status">
      <CheckCircle size={15} />
      {msg}
    </div>
  );
}

export default Toast;
