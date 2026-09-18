import { boundDevice } from "./enrollment.ts";
import type { Environment } from "./index.ts";

type Peer = {
  user: string;
  session: string;
  side?: string;
  mailbox?: string;
  messages?: number;
};
type Mailbox = {
  name: string;
  id: string;
  creator: string;
  approver?: string;
  expires: number;
  messages: { type: "message"; side: string; phase: string; body: string }[];
};
const APP_ID = "io.loofah.sync.pairing.v1";

export class PairingMailbox {
  constructor(
    private readonly state: DurableObjectState,
    private readonly env: Environment,
  ) {}

  async fetch(request: Request) {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket")
      return new Response(null, { status: 426 });
    const user = request.headers.get("x-loofah-user");
    const session = request.headers.get("x-loofah-session");
    if (!user || !session || !(await this.active({ user, session })))
      return new Response(null, { status: 403 });
    if (this.state.getWebSockets().length >= 8)
      return new Response(null, { status: 429 });
    const [client, server] = Object.values(new WebSocketPair());
    this.state.acceptWebSocket(server);
    server.serializeAttachment({ user, session } satisfies Peer);
    server.send(JSON.stringify({ type: "welcome", welcome: {} }));
    const alarm = await this.state.storage.getAlarm();
    await this.state.storage.setAlarm(
      Math.min(alarm ?? Infinity, Date.now() + 600_000),
    );
    return new Response(null, { status: 101, webSocket: client });
  }

  async active(peer: Peer) {
    return this.env.DB.prepare(`SELECT s.id FROM session s JOIN user u ON u.id = s.userId
      JOIN sync_accounts a ON a.user_id = u.id WHERE s.id = ? AND u.id = ? AND u.emailVerified = 1 AND a.active = 1 AND s.expiresAt > ?`)
      .bind(peer.session, peer.user, Date.now())
      .first();
  }

  async webSocketMessage(socket: WebSocket, raw: string | ArrayBuffer) {
    await this.state.blockConcurrencyWhile(async () => {
      const peer: Peer = socket.deserializeAttachment();
      try {
        if (
          typeof raw !== "string" ||
          raw.length > 16_384 ||
          !(await this.active(peer))
        )
          throw new Error();
        peer.messages = (peer.messages ?? 0) + 1;
        if (peer.messages > 128) throw new Error();
        socket.serializeAttachment(peer);
        const message = JSON.parse(raw);
        if (!message || typeof message.type !== "string") throw new Error();
        const send = (value: unknown) => socket.send(JSON.stringify(value));
        const boxes = await this.state.storage.list<Mailbox>({
          prefix: "box:",
        });
        const live = [...boxes.values()].filter(
          (box) => box.expires > Date.now(),
        );
        if (message.type === "bind") {
          if (
            peer.side ||
            message.appid !== APP_ID ||
            typeof message.side !== "string" ||
            !/^[a-f0-9]{10,64}$/.test(message.side)
          )
            throw new Error();
          peer.side = message.side;
          socket.serializeAttachment(peer);
          send({ type: "ack", id: message.id });
          return;
        }
        if (!peer.side) throw new Error();
        send({ type: "ack", id: message.id });
        if (message.type === "allocate") {
          const now = Date.now();
          const previous = await this.state.storage.get<{
            start: number;
            count: number;
          }>("attempts");
          const attempts =
            previous && now - previous.start < 600_000
              ? previous
              : { start: now, count: 0 };
          if (attempts.count >= 10) throw new Error();
          attempts.count++;
          await this.state.storage.put("attempts", attempts);
          if (
            live.length >= 3 ||
            live.some((box) => box.creator === peer.session)
          )
            throw new Error();
          let name: string;
          do {
            name = String(
              (crypto.getRandomValues(new Uint32Array(1))[0] % 1_000_000) + 1,
            );
          } while (live.some((box) => box.name === name));
          const box: Mailbox = {
            name,
            id: crypto.randomUUID(),
            creator: peer.session,
            expires: Date.now() + 600_000,
            messages: [],
          };
          await this.state.storage.put(`box:${name}`, box);
          await this.state.storage.setAlarm(
            Math.min(...live.map((box) => box.expires), box.expires),
          );
          send({ type: "allocated", nameplate: name });
          return;
        }
        if (message.type === "list") {
          if (!(await boundDevice(this.env.DB, peer.user, peer.session)))
            throw new Error();
          send({
            type: "nameplates",
            nameplates: live.map((box) => ({ id: box.name })),
          });
          return;
        }
        if (message.type === "ping") {
          send({ type: "pong", pong: message.ping });
          return;
        }
        if (message.type === "close") {
          if (!peer.mailbox || peer.mailbox !== message.mailbox)
            throw new Error();
          for (const box of live.filter((box) => box.id === peer.mailbox))
            await this.state.storage.delete(`box:${box.name}`);
          send({ type: "closed" });
          socket.close(1000, "Pairing closed");
          return;
        }
        const box =
          message.type === "claim"
            ? live.find((box) => box.name === message.nameplate)
            : live.find((box) => box.id === peer.mailbox);
        if (!box) throw new Error();
        if (
          box.approver &&
          !(await boundDevice(this.env.DB, peer.user, box.approver))
        )
          throw new Error();
        if (message.type === "claim") {
          if (
            peer.mailbox ||
            this.state.getWebSockets().some((client) => {
              const other: Peer = client.deserializeAttachment();
              return other.session === peer.session && other.mailbox === box.id;
            })
          )
            throw new Error();
          if (box.creator !== peer.session) {
            if (
              box.approver ||
              !(await boundDevice(this.env.DB, peer.user, peer.session))
            )
              throw new Error();
            box.approver = peer.session;
          }
          peer.mailbox = box.id;
          socket.serializeAttachment(peer);
          await this.state.storage.put(`box:${box.name}`, box);
          send({ type: "claimed", mailbox: box.id });
        } else if (message.type === "open") {
          if (message.mailbox !== box.id) throw new Error();
          for (const value of box.messages) send(value);
        } else if (message.type === "release") {
          if (message.nameplate !== box.name) throw new Error();
          send({ type: "released" });
        } else if (message.type === "add") {
          if (
            typeof message.phase !== "string" ||
            message.phase.length > 32 ||
            typeof message.body !== "string" ||
            !/^[a-f0-9]+$/.test(message.body) ||
            message.body.length % 2 !== 0
          )
            throw new Error();
          const previous = box.messages.find(
            (value) =>
              value.side === peer.side && value.phase === message.phase,
          );
          if (previous && previous.body !== message.body) throw new Error();
          if (!previous) {
            if (box.messages.length >= 16) throw new Error();
            const value = {
              type: "message" as const,
              side: peer.side,
              phase: message.phase,
              body: message.body,
            };
            box.messages.push(value);
            await this.state.storage.put(`box:${box.name}`, box);
            for (const client of this.state.getWebSockets()) {
              const recipient: Peer = client.deserializeAttachment();
              if (
                recipient.mailbox === box.id &&
                (await this.active(recipient))
              )
                client.send(JSON.stringify(value));
            }
          }
        } else throw new Error();
      } catch {
        socket.send(
          JSON.stringify({
            type: "error",
            error: "Pairing unavailable. Create a new code.",
            orig: {},
          }),
        );
        socket.close(1008, "Pairing unavailable");
      }
    });
  }

  async webSocketClose(socket: WebSocket) {
    await this.state.blockConcurrencyWhile(async () => {
      const peer: Peer = socket.deserializeAttachment();
      const boxes = await this.state.storage.list<Mailbox>({ prefix: "box:" });
      for (const [key, box] of boxes) {
        if (
          box.id === peer.mailbox ||
          (!peer.mailbox && box.creator === peer.session)
        ) {
          await this.state.storage.delete(key);
          for (const other of this.state.getWebSockets()) {
            if (
              other !== socket &&
              (other.deserializeAttachment() as Peer).mailbox === box.id
            )
              other.close(1000, "Pairing cancelled");
          }
        }
      }
    });
  }

  async webSocketError(socket: WebSocket) {
    await this.webSocketClose(socket);
  }

  async alarm() {
    const boxes = await this.state.storage.list<Mailbox>({ prefix: "box:" });
    let next = Infinity;
    for (const [key, box] of boxes) {
      if (box.expires <= Date.now()) {
        await this.state.storage.delete(key);
        for (const socket of this.state.getWebSockets())
          if ((socket.deserializeAttachment() as Peer).mailbox === box.id)
            socket.close(1000, "Pairing expired");
      } else next = Math.min(next, box.expires);
    }
    for (const socket of this.state.getWebSockets())
      if (!(socket.deserializeAttachment() as Peer).mailbox)
        socket.close(1000, "Pairing expired");
    if (Number.isFinite(next)) await this.state.storage.setAlarm(next);
  }
}
