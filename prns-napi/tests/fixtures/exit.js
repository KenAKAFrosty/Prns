const { startNode } = require('../../index.js');

async function main() {
  const node = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['exitfixture'] }] },
    () => {}
  );
  await node.ready();
  await node.attachTcpServer({ bind: '127.0.0.1:14260' });
  await node.stop();
  const guard = setTimeout(() => {
    console.error('fixture-exit-hang');
    process.exit(1);
  }, 5000);
  guard.unref();
  console.log('fixture-exit-ok');
}

main().catch((error) => {
  console.error('fixture-exit-fail', error.code || '', error.message);
  process.exit(1);
});
