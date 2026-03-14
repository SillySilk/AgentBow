/** @type {import('tailwindcss').Config} */
export default {
  content: ["./src/**/*.{html,js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        panel: "#1a1a2e",
        surface: "#16213e",
        user: "#0f3460",
        accent: "#e94560",
        muted: "#a8b2d8",
        border: "#2a2a4a",
      },
    },
  },
  plugins: [],
};
