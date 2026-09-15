/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{vue,js,ts}'],
  corePlugins: { preflight: false },
  theme: {
    extend: {
      colors: {
        brand: {
          50: '#eef8ff',
          500: '#0ea5e9',
          600: '#0284c7',
          700: '#0369a1',
          900: '#0b1220',
        },
      },
    },
  },
  plugins: [],
}
