/**
 * Utility functions for the Oracle ingestion pipeline.
 */

export interface ChunkInput {
  file_id: string;
  text: string;
  chunk_index: number;
  start_char: number;
  end_char: number;
}

export interface ChunkOutput extends ChunkInput {
  id: string;
  kind: string;
  symbol_name: string;
  signature: string;
  line_start: number;
  line_end: number;
  language: string;
  symbols_used: string[];
}

export function chunkId(fileId: string, index: number): string {
  return `${fileId}#chunk-${String(index).padStart(4, "0")}`;
}

export function classifySourceKind(source: string): string {
  const lower = source.toLowerCase();
  if (lower.includes("/tests/") || lower.endsWith(".test.js") || lower.endsWith(".test.ts")) {
    return "test_regression_secondary";
  }
  if (lower.endsWith(".md") || lower.endsWith(".txt") || lower.includes("/docs/")) {
    return "documentation_or_plan_secondary";
  }
  if (lower.endsWith((".js", ".jsx", ".ts", ".tsx", ".py", ".rs", ".kt", ".java"))) {
    return "implementation_primary";
  }
  return "structured_config";
}

export function classifyDomains(source: string, text: string): string[] {
  const haystack = `${source}\n${text}`.toLowerCase();
  const domains: string[] = [];
  if (/output_renders|artifact_url|manifest_url/.test(haystack)) domains.push("rnaseq_output_release");
  if (/scaleway|instance|serverless/.test(haystack)) domains.push("scaleway_provider_console");
  if (/cloudflare|worker|workers/.test(haystack)) domains.push("cloudflare_provider_console");
  if (/zdr|gdpr|privacy/.test(haystack)) domains.push("provider_privacy");
  return domains.length > 0 ? domains : ["general"];
}

export function formatForEmbedding(chunk: ChunkOutput): string {
  const lines: string[] = [
    "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks.",
    `SOURCE_PATH: ${chunk.file_id}`,
    `CHUNK_KIND: ${chunk.kind}`,
    `SYMBOL_NAME: ${chunk.symbol_name}`,
    `LANGUAGE: ${chunk.language}`,
    `LINE_RANGE: L${chunk.line_start}-L${chunk.line_end}`,
    "RAW_CHUNK:",
    chunk.text,
  ];
  return lines.join("\n");
}

export function queryEmbeddingText(query: string): string {
  const lines: string[] = [
    "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks.",
    `QUERY: ${query}`,
  ];
  const domains = classifyDomains("", query);
  if (domains.length > 0 && domains[0] !== "general") {
    lines.push(`QUERY_DOMAIN_TAGS: ${domains.join(", ")}`);
  }
  return lines.join("\n");
}
