package oracle.ingestion;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * Main entry point for the Oracle text chunking pipeline.
 */
public class ChunkPipeline {

    private static final int CODE_MAX_CHARS = 2500;
    private static final int CODE_OVERLAP = 400;

    private final ChunkConfig config;
    private final List<ChunkRecord> records;

    public ChunkPipeline(ChunkConfig config) {
        this.config = config;
        this.records = new ArrayList<>();
    }

    /**
     * Process a single file and produce chunk records.
     *
     * @param filePath the path to the source file
     * @param rootPath the root directory for relative path computation
     * @return list of chunk records
     */
    public List<ChunkRecord> processFile(Path filePath, Path rootPath) {
        String fileId = rootPath.relativize(filePath).toString();
        List<ChunkRecord> result = new ArrayList<>();
        try {
            String text = java.nio.file.Files.readString(filePath);
            List<int[]> windows = splitText(text, config.getMaxChars(), config.getOverlap());
            for (int i = 0; i < windows.size(); i++) {
                int start = windows.get(i)[0];
                int end = windows.get(i)[1];
                String chunkText = text.substring(start, end).trim();
                result.add(new ChunkRecord(
                    fileId + "#chunk-" + String.format("%04d", i),
                    fileId, i, start, end, chunkText,
                    "text_slice", "", "", 0, 0
                ));
            }
        } catch (Exception e) {
            System.err.println("Failed to process " + filePath + ": " + e.getMessage());
        }
        return result;
    }

    private List<int[]> splitText(String text, int maxChars, int overlap) {
        List<int[]> chunks = new ArrayList<>();
        int start = 0;
        String clean = text.replace("\r\n", "\n");
        while (start < clean.length()) {
            int end = Math.min(clean.length(), start + maxChars);
            chunks.add(new int[]{start, end});
            if (end >= clean.length()) break;
            start = Math.max(0, end - overlap);
        }
        return chunks;
    }

    public List<ChunkRecord> getRecords() { return records; }
}
