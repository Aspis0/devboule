/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        cream: {
          50: "#FAF8F5",
          100: "#F5F0EB",
          200: "#EDE8E3",
          300: "#D8D3CE",
          400: "#B5B0AB",
          500: "#8A8580",
          600: "#6B6661",
          700: "#4A4742",
          800: "#2D2A26",
        },
        terracotta: {
          DEFAULT: "#C4956A",
          50: "#FDF8F3",
          100: "#F5E6D8",
          200: "#E8CDB0",
          300: "#D4AD8A",
          400: "#C4956A",
          500: "#A47A52",
          600: "#8A6440",
        },
        sage: {
          DEFAULT: "#7BAE7F",
          light: "#A5CCA8",
          dark: "#5A8E5E",
        },
        amber: {
          DEFAULT: "#D4A853",
          light: "#E4C88A",
          dark: "#B48A3A",
        },
        coral: {
          DEFAULT: "#C47A6A",
          light: "#D9A598",
          dark: "#A45A4A",
        },
        teal: {
          DEFAULT: "#6A9AB5",
          light: "#98BDD0",
          dark: "#4A7A95",
        },
        // Indigo is reserved for the mini-coder MINI chip — a distinct hue not used
        // by any cli/role badge (those use teal/terracotta/sage/cream).
        indigo: {
          DEFAULT: "#6A6AB5",
          light: "#9898D0",
          dark: "#4A4A95",
        },
      },
      /*
       * NOTE: These override Tailwind defaults intentionally.
       * Tailwind defaults: xl=0.75rem(12px), 2xl=1rem(16px), 3xl=1.5rem(24px)
       * Our values:        xl=12px(same), 2xl=16px(same), 3xl=20px(shrunk), 4xl=24px(new)
       * This shifts 3xl down and adds 4xl to maintain the full scale.
       */
      borderRadius: {
        xl: "12px",
        "2xl": "16px",
        "3xl": "20px",
        "4xl": "24px",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "-apple-system", "sans-serif"],
        mono: ["JetBrains Mono", "Fira Code", "monospace"],
      },
      fontSize: {
        "2xs": ["10px", { lineHeight: "14px", letterSpacing: "0.04em" }],
        "label-xs": ["11px", { lineHeight: "15px" }],
        label: ["13px", { lineHeight: "18px" }],
        "body-sm": ["14px", { lineHeight: "20px" }],
      },
      boxShadow: {
        "soft-xs": "0 1px 2px rgba(45, 42, 38, 0.03)",
        "soft-sm": "0 1px 3px rgba(45, 42, 38, 0.05)",
        soft: "0 2px 8px rgba(45, 42, 38, 0.06)",
        "soft-md": "0 4px 16px rgba(45, 42, 38, 0.08)",
        "soft-lg": "0 8px 32px rgba(45, 42, 38, 0.10)",
      },
    },
  },
  plugins: [],
};
