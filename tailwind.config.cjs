module.exports = {
  content: ['./index.html', './src/**/*.{svelte,ts,js}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        ember: '#b54f2e',
        steel: '#255f85',
      },
      fontFamily: {
        display: ['"Chakra Petch"', 'sans-serif'],
        body: ['Manrope', 'sans-serif'],
      },
    },
  },
  plugins: [],
};
