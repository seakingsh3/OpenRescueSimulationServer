const daisyui = require("daisyui");

module.exports = {
  content: ["./templates/**/*.html", "./static/js/**/*.js", "./help/**/*.html"],
  theme: {
    extend: {},
  },
  plugins: [daisyui.default || daisyui],
};
