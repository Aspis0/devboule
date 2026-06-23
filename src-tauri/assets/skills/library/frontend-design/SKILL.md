---
name: frontend-design
description: Enforce frontend design discipline and consistency. Use when building or modifying UI components to maintain visual and accessibility standards.
metadata:
  author: devboule
  version: "1.0"
---
- **Tokens**: Reuse existing design tokens (colors, spacing, typography) instead of new literals.
- **Consistency**: Match established component patterns and visual language.
- **Semantics**: Use semantic HTML (headings, lists, buttons) for accessibility and SEO.
- **Accessibility**: Ensure labeled controls, sufficient contrast, and keyboard focus management.
- **Composition**: Build new UIs by composing existing components; avoid reinventing wheels.
- **No New Deps**: Do not add new fonts, CDNs, or libraries unless strictly necessary and approved.
- **Review**: Check for visual regressions and accessibility compliance (WCAG).
