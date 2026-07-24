import { startNode } from '../../index.js';

const port = process.env.PORT ?? '4242';

const node = startNode(
  {
    destinations: [
      {
        appName: 'hopspot',
        aspects: ['host'],
        announceAppData: Buffer.from('napi-tcp-server-host'),
      },
    ],
  },
  () => {}
);
await node.ready();
await node.attachTcpServer({ bind: `0.0.0.0:${port}` });
console.log(`napi-tcp-server-host: listening on 0.0.0.0:${port}`);

const [destination] = node.destinationHashes;
await node.announce(destination);
setInterval(() => {
  node.announce(destination).catch(() => {});
}, 2000);
