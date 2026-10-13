import StatusMessageView, { type StatusMessage } from "./StatusMessageView";

type Props = {
  mode: "login" | "register";
  setMode: (mode: "login" | "register") => void;
  username: string;
  setUsername: (value: string) => void;
  email: string;
  setEmail: (value: string) => void;
  identifier: string;
  setIdentifier: (value: string) => void;
  password: string;
  setPassword: (value: string) => void;
  submittingAuth: boolean;
  authStatus: StatusMessage | null;
  handleRegister: () => Promise<void>;
  handleLogin: () => Promise<void>;
};

export default function AuthScreen({
  mode, setMode, username, setUsername, email, setEmail, identifier, setIdentifier, password, setPassword, submittingAuth, authStatus, handleRegister, handleLogin
}: Props) {
  return (
    <div className="container">
      <h1>FinanceTracker</h1>

      <div className="tabs">
        <button type="button" className={mode === "login" ? "active" : ""} onClick={() => setMode("login")}>
          Login
        </button>
        <button type="button" className={mode === "register" ? "active" : ""} onClick={() => setMode("register")}>
          Register
        </button>
      </div>

      {mode === "register" ? (
        <div className="card">
          <form className="formGrid" onSubmit={(e) => { e.preventDefault(); void handleRegister(); }}>
            <label htmlFor="register-username">Username</label>
            <input
              id="register-username"
              autoComplete="username"
              required
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />

            <label htmlFor="register-email">Email</label>
            <input
              id="register-email"
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />

            <label htmlFor="register-password">Password</label>
            <input
              id="register-password"
              type="password"
              autoComplete="new-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />

            <div className="fullRow">
              <button type="submit" disabled={submittingAuth}>
                {submittingAuth ? "Creating account…" : "Create account"}
              </button>
            </div>
          </form>
        </div>
      ) : (
        <div className="card">
          <form className="formGrid" onSubmit={(e) => { e.preventDefault(); void handleLogin(); }}>
            <label htmlFor="login-identifier">Username or Email</label>
            <input
              id="login-identifier"
              autoComplete="username"
              required
              value={identifier}
              onChange={(e) => setIdentifier(e.target.value)}
            />

            <label htmlFor="login-password">Password</label>
            <input
              id="login-password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />

            <div className="fullRow">
              <button type="submit" disabled={submittingAuth}>
                {submittingAuth ? "Logging in…" : "Login"}
              </button>
            </div>
          </form>
        </div>
      )}

      <StatusMessageView message={authStatus} />
    </div>
  );
}
