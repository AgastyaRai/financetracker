export type StatusMessage = {
  text: string;
  type: "error" | "success";
};

export default function StatusMessageView({ message }: { message: StatusMessage | null }) {
  if (!message) return null;

  return (
    <p className={`status ${message.type}`} role={message.type === "error" ? "alert" : "status"}>
      {message.text}
    </p>
  );
}
