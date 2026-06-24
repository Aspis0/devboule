import "./work.css";
import type { CensorStripModel } from "./censorStripModel";

export interface CensorStripProps {
  model: CensorStripModel;
  onSelectFile?: (file: string) => void;
}

const getBasename = (path: string) => path.split("/").pop() || path;

export function CensorStrip(props: CensorStripProps) {
  const { model, onSelectFile } = props;
  const dirtyFiles = model.dirtyFiles ?? 0;
  const openFindings = model.openFindings ?? 0;

  const MAX_CHIPS = 14;
  const shown = model.items.slice(0, MAX_CHIPS);

  const summaryText = dirtyFiles > 0
    ? `${dirtyFiles} dirty · ${openFindings} findings`
    : "all clean";

  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        height: "auto",
        padding: "8px 14px",
        borderTop: "1px solid #EFE7DA",
        backgroundColor: "#FBF8F2",
        color: "#3B362F",
        alignItems: "center",
      }}
    >
      <span className="pp-mono" style={{ color: "#8C8578", letterSpacing: "0.05em", marginRight: "12px" }}>
        CENSOR
      </span>

      {model.items.length === 0 ? (
        <span style={{ color: "#9c9488" }}>all clean · no open findings</span>
      ) : (
        <>
          {shown.map((item) => {
            const basename = getBasename(item.file);
            const isDirty = item.status === "dirty";
            const glyphColor = isDirty ? "#C2542F" : "#7FA468";

            return (
              <span
                key={item.file}
                data-censor-file={item.file}
                data-censor-status={item.status}
                onClick={() => onSelectFile?.(item.file)}
                style={{
                  cursor: onSelectFile ? "pointer" : "default",
                  display: "inline-flex",
                  alignItems: "center",
                  gap: "5px",
                  padding: "3px 8px",
                  margin: "0 4px",
                  borderRadius: "4px",
                  backgroundColor: isDirty ? "#FDF5F3" : "#F4F9F0",
                  border: "1px solid #EFE7DA",
                }}
              >
                {isDirty ? (
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" style={{ fill: glyphColor }}>
                    <path d="M6 1L11 10H1L6 1Z" />
                  </svg>
                ) : (
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" style={{ fill: glyphColor }}>
                    <path d="M2 6L5 9L10 3" stroke={glyphColor} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                )}
                <span className="pp-mono">{basename}</span>
                <span className="pp-mono" style={{ color: glyphColor }}>
                  {isDirty ? item.openCount : "CLEAN"}
                </span>
              </span>
            );
          })}
          {model.items.length > MAX_CHIPS && (
            <span className="pp-mono" style={{ color: "#9c9488", marginLeft: "4px" }}>
              +{model.items.length - MAX_CHIPS} more
            </span>
          )}
        </>
      )}

      <span className="pp-mono" style={{ color: "#9c9488", marginLeft: "auto" }}>
        {summaryText}
      </span>
    </div>
  );
}
