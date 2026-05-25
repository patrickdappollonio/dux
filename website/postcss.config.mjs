// Tailwind 4 via PostCSS. We use the PostCSS path (rather than the Vite plugin)
// for broader compatibility with the current Vite/Rolldown resolver bindings.
export default {
  plugins: {
    "@tailwindcss/postcss": {},
  },
};
