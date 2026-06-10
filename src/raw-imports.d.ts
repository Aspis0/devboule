// Vite `?raw` imports return the file's text content as a string. Declared here
// because this tsconfig does not pull in `vite/client` types. Used by the
// Work-mode "no second poller" static source-string assertion test.
declare module "*?raw" {
  const content: string;
  export default content;
}
