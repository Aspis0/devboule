import React, { useState, useEffect, useCallback } from "react";

interface ChunkData {
  id: string;
  file_id: string;
  text: string;
  kind: string;
  symbol_name: string;
  language: string;
  score: number;
}

interface SearchResults {
  results: ChunkData[];
  query: string;
  total: number;
}

export function OracleSearchPanel(): JSX.Element {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResults | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [domainFilter, setDomainFilter] = useState<string>("");

  const handleSearch = useCallback(async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const params = new URLSearchParams({ q: query });
      if (domainFilter) params.set("domain", domainFilter);
      const response = await fetch(`/api/context?${params}`);
      if (!response.ok) throw new Error(`Search failed: ${response.status}`);
      const data = await response.json();
      setResults({
        results: data.results || [],
        query: data.query,
        total: data.results?.length || 0,
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, [query, domainFilter]);

  useEffect(() => {
    const debounceTimer = setTimeout(() => {
      if (query.length >= 3) handleSearch();
    }, 300);
    return () => clearTimeout(debounceTimer);
  }, [query, handleSearch]);

  return (
    <div className="oracle-search-panel">
      <div className="search-bar">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search Oracle chunks..."
          aria-label="Search query"
        />
        <select value={domainFilter} onChange={(e) => setDomainFilter(e.target.value)}>
          <option value="">All domains</option>
          <option value="rnaseq">RNA-seq</option>
          <option value="scaleway">Scaleway</option>
          <option value="cloudflare">Cloudflare</option>
          <option value="oracle">Oracle</option>
        </select>
        <button onClick={handleSearch} disabled={loading}>
          {loading ? "Searching..." : "Search"}
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      {results && (
        <div className="results">
          <p>{results.total} results for &quot;{results.query}&quot;</p>
          {results.results.map((chunk) => (
            <ChunkResult key={chunk.id} chunk={chunk} />
          ))}
        </div>
      )}
    </div>
  );
}

function ChunkResult({ chunk }: { chunk: ChunkData }): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const preview = chunk.text.slice(0, 200) + (chunk.text.length > 200 ? "..." : "");
  return (
    <div className="chunk-result" onClick={() => setExpanded(!expanded)}>
      <div className="chunk-header">
        <span className="chunk-kind">{chunk.kind}</span>
        <span className="chunk-symbol">{chunk.symbol_name || chunk.file_id}</span>
        <span className="chunk-score">{chunk.score.toFixed(3)}</span>
      </div>
      <pre className="chunk-preview">{expanded ? chunk.text : preview}</pre>
    </div>
  );
}

export default OracleSearchPanel;
