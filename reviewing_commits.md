<h1 align="center">Tracking read/unread commits</h1>

Once upon a time, I would pull master and check to see what had come down
the pipe, like this:

```
$ git pull
$ git log ORIG_HEAD..
```

This works, but if you don't look through the new commits then-and-there you
can lose track of which ones you've looked at.  I wanted a way to track the
"read/unread" status of each commit, like I do with email.

## Enter git-notes

In fact, git gives you a way to leave notes for yourself attached to commits.
It's called... [`git notes`].  You use it like this:

```
$ git notes add b76598d -m 'Reviewed'
```

So, I started marking the commits I'd looked at by attaching a note to them.
Now, the notes show up in `git log` and I can see which commits I've looked
at, and which are still waiting to be reviewed.

```
$ git show b76598d
commit b76598d2028868fe70d0e038a2841caa6e477d23
Author: Joe Smith <joe@smith.net>
Date:   Fri Jan 8 08:45:16 2021 +0900

    Switch from gcc-9 to gcc-10 on macOS

Notes:
    Reviewed
```

Tips:

* If you make a typo, you can edit a note interactively like so:
  `git notes edit <commit>`
* You can namespace your notes with `--ref` if you want to keep them
  organised somehow.
* If you make your notes look like a ["trailer"], some programs (like tig)
  highlight them nicely.

And for my #1 tip: If you use tig you can set up a keybind by putting this
in your gitconfig:

  ```
  [tig "bind"]
      generic = T >git notes add %(commit) -m 'Reviewed'
  ```

Now, hitting 'T' will mark the highlighted commit as reviewed.  This makes
the whole review process really smooth.  (This keybind is even suggested
specifically in the tig manual.)

## Making a system

The way I think of my "reviewed" notes is this: it's like the read/unread
status on my emails.  It doesn't imply approval, disapproval, or any judgement
whatsoever.  It just means that I've looked at the commit.

If I've actually built a commit and tried it out, I write "tested" instead.

...and that's about as far as my system goes!  Of course, you're free to
make yours as complicated as you like.

And remember: these comments aren't indended to be seen by anyone else;
they're just for your own personal use.  If you want to give authors feedback
about thier commits, you should use github/gitlab/trac/gerrit/redmine/the
mailing list/whatever.  This is all about _local, private_ review tracking.

[`git notes`]: https://git-scm.com/docs/git-notes
["trailer"]: https://git-scm.com/docs/git-interpret-trailers

(Next: [Reviewing merge requests](reviewing_mrs.md).)
