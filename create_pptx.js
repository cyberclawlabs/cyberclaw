const pptxgen = require("pptxgenjs");

async function main() {
  const pptx = new pptxgen();
  pptx.addSlide().addText("Hello World", { x: 1, y: 2, fontSize: 36 });
  await pptx.writeFile({ fileName: "/tmp/test.pptx" });
  console.log("Done: /tmp/test.pptx");
}

main();
