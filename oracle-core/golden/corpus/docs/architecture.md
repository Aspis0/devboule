# Oracle Architecture - Distributed Code Intelligence Platform

## 1. Ingestion Pipeline

### 1.1 Core Architecture

The ingestion pipeline subsystem is a critical component of the Oracle architecture. It handles the processing of source code files across multiple programming languages including Rust, Python, TypeScript, JavaScript, Java, and Kotlin. The system uses a sophisticated chunking strategy that combines AST-aware semantic splitting with sliding window fallback for files that lack parseable definition boundaries.

Each chunk is annotated with structured metadata including its semantic kind (function, struct, enum, class, module, etc.), symbol name, signature, line range, language, and cross-references to imported symbols. This metadata is used both for embedding generation (providing context headers that improve retrieval quality) and for the extractive answer synthesis that produces grounded responses without requiring an LLM call.

The embedding pipeline uses Qwen3-Embedding-0.6B with a 1024-dimensional output space. Each chunk embedding text is prefixed with metadata headers including SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER. These structured prefixes ensure that the embedding model captures the semantic context of each chunk beyond its raw text content.

Domain classification uses keyword-based heuristics to tag chunks with domain labels such as rnaseq_output_release, scaleway_provider_console, cloudflare_provider_console, oracle_indexing, oracle_answering, provider_privacy, and projects_mini_notion. These domain tags drive query-time bonus scoring that boosts relevant chunks when the user query matches specific domain patterns. For example, a query containing both scaleway and gpu terms activates the domain_mechanism_bonus function, which applies extra scoring weight to chunks from Scaleway-related source files that mention GPU lifecycle keywords.

The pipeline processes files in configurable batches, with adaptive batch sizing based on available system memory. Files are first classified by extension into code, documentation, structured data, or default profiles, each with its own chunking parameters. The ingestion manifest tracks file signatures (size, mtime, chunk profile version) to enable incremental re-indexing that only processes files whose content has actually changed since the last run.

### 1.1 Core Architecture

The ingestion pipeline subsystem is a critical component of the Oracle architecture. It handles the processing of source code files across multiple programming languages including Rust, Python, TypeScript, JavaScript, Java, and Kotlin. The system uses a sophisticated chunking strategy that combines AST-aware semantic splitting with sliding window fallback for files that lack parseable definition boundaries.

Each chunk is annotated with structured metadata including its semantic kind (function, struct, enum, class, module, etc.), symbol name, signature, line range, language, and cross-references to imported symbols. This metadata is used both for embedding generation (providing context headers that improve retrieval quality) and for the extractive answer synthesis that produces grounded responses without requiring an LLM call.

The embedding pipeline uses Qwen3-Embedding-0.6B with a 1024-dimensional output space. Each chunk embedding text is prefixed with metadata headers including SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER. These structured prefixes ensure that the embedding model captures the semantic context of each chunk beyond its raw text content.

Domain classification uses keyword-based heuristics to tag chunks with domain labels such as rnaseq_output_release, scaleway_provider_console, cloudflare_provider_console, oracle_indexing, oracle_answering, provider_privacy, and projects_mini_notion. These domain tags drive query-time bonus scoring that boosts relevant chunks when the user query matches specific domain patterns. For example, a query containing both scaleway and gpu terms activates the domain_mechanism_bonus function, which applies extra scoring weight to chunks from Scaleway-related source files that mention GPU lifecycle keywords.

The pipeline processes files in configurable batches, with adaptive batch sizing based on available system memory. Files are first classified by extension into code, documentation, structured data, or default profiles, each with its own chunking parameters. The ingestion manifest tracks file signatures (size, mtime, chunk profile version) to enable incremental re-indexing that only processes files whose content has actually changed since the last run.

### 1.1 Core Architecture

The ingestion pipeline subsystem is a critical component of the Oracle architecture. It handles the processing of source code files across multiple programming languages including Rust, Python, TypeScript, JavaScript, Java, and Kotlin. The system uses a sophisticated chunking strategy that combines AST-aware semantic splitting with sliding window fallback for files that lack parseable definition boundaries.

Each chunk is annotated with structured metadata including its semantic kind (function, struct, enum, class, module, etc.), symbol name, signature, line range, language, and cross-references to imported symbols. This metadata is used both for embedding generation (providing context headers that improve retrieval quality) and for the extractive answer synthesis that produces grounded responses without requiring an LLM call.

The embedding pipeline uses Qwen3-Embedding-0.6B with a 1024-dimensional output space. Each chunk embedding text is prefixed with metadata headers including SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER. These structured prefixes ensure that the embedding model captures the semantic context of each chunk beyond its raw text content.

Domain classification uses keyword-based heuristics to tag chunks with domain labels such as rnaseq_output_release, scaleway_provider_console, cloudflare_provider_console, oracle_indexing, oracle_answering, provider_privacy, and projects_mini_notion. These domain tags drive query-time bonus scoring that boosts relevant chunks when the user query matches specific domain patterns. For example, a query containing both scaleway and gpu terms activates the domain_mechanism_bonus function, which applies extra scoring weight to chunks from Scaleway-related source files that mention GPU lifecycle keywords.

The pipeline processes files in configurable batches, with adaptive batch sizing based on available system memory. Files are first classified by extension into code, documentation, structured data, or default profiles, each with its own chunking parameters. The ingestion manifest tracks file signatures (size, mtime, chunk profile version) to enable incremental re-indexing that only processes files whose content has actually changed since the last run.

## 2. Chunking Strategy

### 2.1 Semantic Splitting

The chunking strategy subsystem implements a multi-tier approach to source code segmentation. For code files (identified by language-specific extensions), the system first attempts AST-aware semantic splitting via chunk_file_semantically, which scans for top-level definition boundaries (functions, classes, structs, enums, traits, modules) and groups lines between these boundaries into coherent semantic units. When a single definition exceeds the maximum chunk size, the system applies sub-splitting at logical points such as blank lines or comment blocks.

For files where semantic chunking produces fewer than two chunks (indicating no meaningful definition boundaries were found), the system falls back to a sliding-window approach via split_text, which divides the text at newline boundaries with configurable overlap. This fallback ensures that even plain configuration files, data files, and code without standard definitions are properly chunked for embedding and retrieval.

The chunking profiles are configurable per file type: code files use a 2500-character maximum with 400-character overlap, documentation files use 12000 characters with 1200-character overlap, and structured data files use 8000 characters with 900-character overlap. Each chunk receives a unique identifier in the format {file_id}#chunk-{index:04d} and carries metadata including the chunking kind (function, struct, class, section, text_slice), symbol name, signature text, line range, language, and a JSON-serialized list of referenced symbols.

The semantic chunker normalizes line endings (CRLF to LF) before processing, ensuring consistent behavior across platforms. For languages without definition patterns (markdown, YAML, JSON, etc.), the fallback chunker splits at blank-line and heading boundaries, producing section-level chunks that preserve document structure. The sub-splitting algorithm for oversized definitions identifies natural break points at blank lines, comment blocks, and statement group boundaries, maintaining semantic coherence within each sub-chunk.

### 2.1 Semantic Splitting

The chunking strategy subsystem implements a multi-tier approach to source code segmentation. For code files (identified by language-specific extensions), the system first attempts AST-aware semantic splitting via chunk_file_semantically, which scans for top-level definition boundaries (functions, classes, structs, enums, traits, modules) and groups lines between these boundaries into coherent semantic units. When a single definition exceeds the maximum chunk size, the system applies sub-splitting at logical points such as blank lines or comment blocks.

For files where semantic chunking produces fewer than two chunks (indicating no meaningful definition boundaries were found), the system falls back to a sliding-window approach via split_text, which divides the text at newline boundaries with configurable overlap. This fallback ensures that even plain configuration files, data files, and code without standard definitions are properly chunked for embedding and retrieval.

The chunking profiles are configurable per file type: code files use a 2500-character maximum with 400-character overlap, documentation files use 12000 characters with 1200-character overlap, and structured data files use 8000 characters with 900-character overlap. Each chunk receives a unique identifier in the format {file_id}#chunk-{index:04d} and carries metadata including the chunking kind (function, struct, class, section, text_slice), symbol name, signature text, line range, language, and a JSON-serialized list of referenced symbols.

The semantic chunker normalizes line endings (CRLF to LF) before processing, ensuring consistent behavior across platforms. For languages without definition patterns (markdown, YAML, JSON, etc.), the fallback chunker splits at blank-line and heading boundaries, producing section-level chunks that preserve document structure. The sub-splitting algorithm for oversized definitions identifies natural break points at blank lines, comment blocks, and statement group boundaries, maintaining semantic coherence within each sub-chunk.

### 2.1 Semantic Splitting

The chunking strategy subsystem implements a multi-tier approach to source code segmentation. For code files (identified by language-specific extensions), the system first attempts AST-aware semantic splitting via chunk_file_semantically, which scans for top-level definition boundaries (functions, classes, structs, enums, traits, modules) and groups lines between these boundaries into coherent semantic units. When a single definition exceeds the maximum chunk size, the system applies sub-splitting at logical points such as blank lines or comment blocks.

For files where semantic chunking produces fewer than two chunks (indicating no meaningful definition boundaries were found), the system falls back to a sliding-window approach via split_text, which divides the text at newline boundaries with configurable overlap. This fallback ensures that even plain configuration files, data files, and code without standard definitions are properly chunked for embedding and retrieval.

The chunking profiles are configurable per file type: code files use a 2500-character maximum with 400-character overlap, documentation files use 12000 characters with 1200-character overlap, and structured data files use 8000 characters with 900-character overlap. Each chunk receives a unique identifier in the format {file_id}#chunk-{index:04d} and carries metadata including the chunking kind (function, struct, class, section, text_slice), symbol name, signature text, line range, language, and a JSON-serialized list of referenced symbols.

The semantic chunker normalizes line endings (CRLF to LF) before processing, ensuring consistent behavior across platforms. For languages without definition patterns (markdown, YAML, JSON, etc.), the fallback chunker splits at blank-line and heading boundaries, producing section-level chunks that preserve document structure. The sub-splitting algorithm for oversized definitions identifies natural break points at blank lines, comment blocks, and statement group boundaries, maintaining semantic coherence within each sub-chunk.

## 3. Embedding Generation

### 3.1 Semantic Prefix Profile

The embedding generation pipeline transforms each chunk into a rich textual representation that combines structured metadata headers with the raw chunk content. The semantic-prefix-v2 profile prepends each chunk with a standardized header block containing TASK, SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER fields.

The TASK header provides a universal instruction to the embedding model about the retrieval purpose. SOURCE_KIND classifies files into categories like implementation_primary, documentation_or_plan_secondary, test_regression_secondary, generated_low_priority, and structured_config, which drive priority hints for query-time ranking. DOMAIN_TAGS are computed by classify_domains, a keyword-based function that matches source path and text content against known domain signal terms to tag chunks with labels like rnaseq_output_release, cloudflare_worker_secret_rotation, and provider_privacy.

The QUESTIONS_THIS_CHUNK_CAN_ANSWER field generates candidate questions based on the domain tags and symbol names, providing the embedding model with query-relevance signals. Questions are template-generated from a mapping of domain labels to question patterns, ensuring that chunks tagged with rnaseq_output_release receive questions about RNA-seq output release flows, while chunks tagged with scaleway_provider_console receive questions about Scaleway dashboard and instance management.

The embedding batch processor groups chunks into batches optimized for the Qwen3-Embedding-0.6B model's encode batch size, balancing throughput against memory pressure. Each batch's aggregate character count is bounded by a configurable budget to prevent OOM conditions during the forward pass. The system monitors GPU temperature and free memory between batches, pausing and resuming automatically when thermal or memory thresholds are approached.

### 3.1 Semantic Prefix Profile

The embedding generation pipeline transforms each chunk into a rich textual representation that combines structured metadata headers with the raw chunk content. The semantic-prefix-v2 profile prepends each chunk with a standardized header block containing TASK, SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER fields.

The TASK header provides a universal instruction to the embedding model about the retrieval purpose. SOURCE_KIND classifies files into categories like implementation_primary, documentation_or_plan_secondary, test_regression_secondary, generated_low_priority, and structured_config, which drive priority hints for query-time ranking. DOMAIN_TAGS are computed by classify_domains, a keyword-based function that matches source path and text content against known domain signal terms to tag chunks with labels like rnaseq_output_release, cloudflare_worker_secret_rotation, and provider_privacy.

The QUESTIONS_THIS_CHUNK_CAN_ANSWER field generates candidate questions based on the domain tags and symbol names, providing the embedding model with query-relevance signals. Questions are template-generated from a mapping of domain labels to question patterns, ensuring that chunks tagged with rnaseq_output_release receive questions about RNA-seq output release flows, while chunks tagged with scaleway_provider_console receive questions about Scaleway dashboard and instance management.

The embedding batch processor groups chunks into batches optimized for the Qwen3-Embedding-0.6B model's encode batch size, balancing throughput against memory pressure. Each batch's aggregate character count is bounded by a configurable budget to prevent OOM conditions during the forward pass. The system monitors GPU temperature and free memory between batches, pausing and resuming automatically when thermal or memory thresholds are approached.

### 3.1 Semantic Prefix Profile

The embedding generation pipeline transforms each chunk into a rich textual representation that combines structured metadata headers with the raw chunk content. The semantic-prefix-v2 profile prepends each chunk with a standardized header block containing TASK, SOURCE_PATH, FILE_NAME, EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME, LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS, and QUESTIONS_THIS_CHUNK_CAN_ANSWER fields.

The TASK header provides a universal instruction to the embedding model about the retrieval purpose. SOURCE_KIND classifies files into categories like implementation_primary, documentation_or_plan_secondary, test_regression_secondary, generated_low_priority, and structured_config, which drive priority hints for query-time ranking. DOMAIN_TAGS are computed by classify_domains, a keyword-based function that matches source path and text content against known domain signal terms to tag chunks with labels like rnaseq_output_release, cloudflare_worker_secret_rotation, and provider_privacy.

The QUESTIONS_THIS_CHUNK_CAN_ANSWER field generates candidate questions based on the domain tags and symbol names, providing the embedding model with query-relevance signals. Questions are template-generated from a mapping of domain labels to question patterns, ensuring that chunks tagged with rnaseq_output_release receive questions about RNA-seq output release flows, while chunks tagged with scaleway_provider_console receive questions about Scaleway dashboard and instance management.

The embedding batch processor groups chunks into batches optimized for the Qwen3-Embedding-0.6B model's encode batch size, balancing throughput against memory pressure. Each batch's aggregate character count is bounded by a configurable budget to prevent OOM conditions during the forward pass. The system monitors GPU temperature and free memory between batches, pausing and resuming automatically when thermal or memory thresholds are approached.

## 4. Query Processing

### 4.1 Lexical Scoring

The query processing pipeline combines lexical term matching with domain-specific bonus scoring to rank chunks by relevance. Query terms are extracted via query_terms, which tokenizes the input, filters stopwords, and returns terms of length three or more. These terms are then matched against chunk text using lexical_chunk_score, which computes a weighted term-frequency score with source quality bonuses.

The scoring system applies ten domain-bonus functions that boost chunks when both the query and chunk content match specific domain patterns. For example, domain_mechanism_bonus adds weight when a query mentions GPU/CPU/VM terms and the chunk is from a Scaleway-related source file. rnaseq_output_release_bonus boosts chunks containing output_renders, artifact_url, and manifest_url when the query asks about RNA-seq outputs. These bonuses are additive and domain-specific, ensuring that highly relevant chunks receive substantially higher scores than generic matches.

The final ranking combines lexical scores with chunk metadata to select the top-N most relevant chunks for the answer prompt. The prepared_context function filters, deduplicates, and formats these chunks into a structured context block that the answer generation model uses to produce grounded responses. Secret redaction via redact_secret_tokens ensures that AWS keys, GitHub tokens, Slack tokens, JWT strings, and other sensitive patterns are replaced with [redacted-secret] before the context reaches the LLM.

The answer generation prompt instructs the LLM to use only the provided context chunks, cite specific chunk references, and return a structured JSON response with answer text, citation list, not_found flag, and suggested_path. When the LLM cannot produce a grounded answer, the system falls back to extractive synthesis, which selects the best sentences from the top-ranked chunks and combines them into a coherent response without requiring LLM generation.

### 4.1 Lexical Scoring

The query processing pipeline combines lexical term matching with domain-specific bonus scoring to rank chunks by relevance. Query terms are extracted via query_terms, which tokenizes the input, filters stopwords, and returns terms of length three or more. These terms are then matched against chunk text using lexical_chunk_score, which computes a weighted term-frequency score with source quality bonuses.

The scoring system applies ten domain-bonus functions that boost chunks when both the query and chunk content match specific domain patterns. For example, domain_mechanism_bonus adds weight when a query mentions GPU/CPU/VM terms and the chunk is from a Scaleway-related source file. rnaseq_output_release_bonus boosts chunks containing output_renders, artifact_url, and manifest_url when the query asks about RNA-seq outputs. These bonuses are additive and domain-specific, ensuring that highly relevant chunks receive substantially higher scores than generic matches.

The final ranking combines lexical scores with chunk metadata to select the top-N most relevant chunks for the answer prompt. The prepared_context function filters, deduplicates, and formats these chunks into a structured context block that the answer generation model uses to produce grounded responses. Secret redaction via redact_secret_tokens ensures that AWS keys, GitHub tokens, Slack tokens, JWT strings, and other sensitive patterns are replaced with [redacted-secret] before the context reaches the LLM.

The answer generation prompt instructs the LLM to use only the provided context chunks, cite specific chunk references, and return a structured JSON response with answer text, citation list, not_found flag, and suggested_path. When the LLM cannot produce a grounded answer, the system falls back to extractive synthesis, which selects the best sentences from the top-ranked chunks and combines them into a coherent response without requiring LLM generation.

### 4.1 Lexical Scoring

The query processing pipeline combines lexical term matching with domain-specific bonus scoring to rank chunks by relevance. Query terms are extracted via query_terms, which tokenizes the input, filters stopwords, and returns terms of length three or more. These terms are then matched against chunk text using lexical_chunk_score, which computes a weighted term-frequency score with source quality bonuses.

The scoring system applies ten domain-bonus functions that boost chunks when both the query and chunk content match specific domain patterns. For example, domain_mechanism_bonus adds weight when a query mentions GPU/CPU/VM terms and the chunk is from a Scaleway-related source file. rnaseq_output_release_bonus boosts chunks containing output_renders, artifact_url, and manifest_url when the query asks about RNA-seq outputs. These bonuses are additive and domain-specific, ensuring that highly relevant chunks receive substantially higher scores than generic matches.

The final ranking combines lexical scores with chunk metadata to select the top-N most relevant chunks for the answer prompt. The prepared_context function filters, deduplicates, and formats these chunks into a structured context block that the answer generation model uses to produce grounded responses. Secret redaction via redact_secret_tokens ensures that AWS keys, GitHub tokens, Slack tokens, JWT strings, and other sensitive patterns are replaced with [redacted-secret] before the context reaches the LLM.

The answer generation prompt instructs the LLM to use only the provided context chunks, cite specific chunk references, and return a structured JSON response with answer text, citation list, not_found flag, and suggested_path. When the LLM cannot produce a grounded answer, the system falls back to extractive synthesis, which selects the best sentences from the top-ranked chunks and combines them into a coherent response without requiring LLM generation.

