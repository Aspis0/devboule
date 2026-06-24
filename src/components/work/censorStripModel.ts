import type { CensorFinding } from "../../types/backend";

export type CensorFileStatus = "clean" | "dirty";

export interface CensorStripItem {
  file: string;
  status: CensorFileStatus;
  openCount: number;
}

export interface CensorStripModel {
  items: CensorStripItem[];
  dirtyFiles: number;
  cleanFiles: number;
  openFindings: number;
}

export function buildCensorStrip(findings: CensorFinding[]): CensorStripModel {
  if (findings.length === 0) {
    return { items: [], dirtyFiles: 0, cleanFiles: 0, openFindings: 0 };
  }

  const fileOpenCounts = new Map<string, number>();
  for (const f of findings) {
    const key = (f.file ?? "").trim() || "(unknown file)";
    if (!fileOpenCounts.has(key)) {
      fileOpenCounts.set(key, 0);
    }
    if (f.disposition === "open") {
      fileOpenCounts.set(key, fileOpenCounts.get(key)! + 1);
    }
  }

  const items: CensorStripItem[] = Array.from(fileOpenCounts.entries()).map(([file, openCount]) => ({
    file,
    status: openCount > 0 ? "dirty" : "clean",
    openCount,
  }));

  items.sort((a, b) => {
    if (a.status !== b.status) {
      return a.status === "dirty" ? -1 : 1;
    }
    return a.file.localeCompare(b.file, "en", { sensitivity: "base" });
  });

  const dirtyFiles = items.filter((i) => i.status === "dirty").length;
  const cleanFiles = items.filter((i) => i.status === "clean").length;
  const openFindings = items.reduce((sum, i) => sum + i.openCount, 0);

  return { items, dirtyFiles, cleanFiles, openFindings };
}
