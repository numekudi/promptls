vim.lsp.config("promptls", {
  cmd = { "promptls" },
  filetypes = { "markdown" },
  -- Attach only to temp-dir .md files (the Ctrl+G buffers), not ordinary Markdown.
  root_dir = function(bufnr, on_dir)
    local name = vim.api.nvim_buf_get_name(bufnr)
    if not name:match("%.md$") then return end
    -- Both CLIs use $TMPDIR if set, else /tmp (Node os.tmpdir / Rust temp_dir).
    -- Compare realpaths too: on macOS /tmp is a symlink to /private/tmp.
    local tmpdir = (os.getenv("TMPDIR") or "/tmp"):gsub("/*$", "/")
    local real_tmpdir = (vim.uv.fs_realpath(tmpdir) or tmpdir):gsub("/*$", "/")
    local real_name = vim.uv.fs_realpath(name) or name
    if vim.startswith(name, tmpdir) or vim.startswith(real_name, real_tmpdir) then
      on_dir(vim.fn.getcwd())
    end
  end,
})
vim.lsp.enable("promptls")

