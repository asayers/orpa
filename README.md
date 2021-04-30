<h1 align="center">Tracking read/unread commits</h1>

**tl;dr**: Do you try to review your colleagues' commits, but sometimes they
slip past you?  Are you tired of re-reviewing commits you've already looked at?
Keep track of the ones you've reviewed and you'll see them exactly once!

## A tale of many reviews

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
* You can set up a keybind in tig like so:
  ```
  [tig "bind"] generic = T !git notes add %(commit) -m 'Reviewed'
  ```
  This makes the whole review process really smooth.
* If you make your notes look like a ["trailer"], some programs (like tig)
  highlight them nicely.

## Making a system

The way I think of my "reviewed" notes is this: it's like the read/unread
status on my emails.  It doesn't imply approval, disapproval, or any judgement
whatsoever.  It just means that I've looked at the commit.

If I've actually built a commit and tried it out, I write "tested" instead.

...and that's about as far as my system goes!  Of course, you're free to
make yours as complicated as you like.

And remember: these comments aren't indended to be seen by anyone else;
they're just for my own personal use.  If you want to give authors feedback
about thier commits, you should use github/gitlab/trac/gerrit/redmine/the
mailing list/whatever.  This is all about _local, private_ review tracking.

[`git notes`]: https://git-scm.com/docs/git-notes
["trailer"]: https://git-scm.com/docs/git-interpret-trailers

## Reviewing merge requests

Reviewing commits once they're been merged is good, but I'd also like to
review incoming changes before they land.  Fortunately, `git notes` allows
you to attach comments to _any_ commit in your repo, whether it's merged to
a local branch or just part of a remote branch.  That means we can use our
system to keep track of which MRs we've looked at.

So, Joe has an MR where he wants to merge 563e5fb..aadb1f9 into master.
I look through those commits, marking them as reviewed.  I send Joe some
feedback.

## Re-reviewing merge requests

Joe has updated his MR, and now it shows the range 7be3424..de31ea2.  I don't
want to look over these commits again - they're very similar to the ones
I've already reviewed.

Once again, git has us covered; this time it's with `git range-diff`:

```
$ git range-diff 563e5fb..aadb1f9 7be3424..de31ea2
1:  9fbc3f82 = 1:  ce0ad59e Make the notes ref configurable
2:  aadb1f9e = 2:  30bb419c Use Lazy for CLI opts
-:  -------- > 3:  de31ea2c Rename --hidden to --all
```

It looks like Joe just rebased and added a commit.  So I can just mark
the first two as seen without thinking, and then take a closer look at the
third one.

## Merged merge requests

Joe has merged his MR to master; however, he did a quick rebase first.
This means that the commits which landed in master don't have notes attached
to them.  If I'm properly awake then I'll notice that the "unreviewed" commits
in master are similar to the ones I reviewed in the MR, use `git range-diff`
to confirm, and then mark them as reviewed.  But this is too much thinking
for my liking!

## Orpa

`orpa` is a tool for streamlining this workflow.  It shows you the commits
which don't yet have any notes attached.

```
$ orpa status
Current branch: The following commits are awaiting review:

  7cc8026 Use the gitlab raw Query API
  251ec84 Replace coloured with yansi
  da05da1 Document the CLI options

Review them using "orpa review"
```

And it has a "review" mode which lets you quickly blast through the unreviewed
commits, marking them with comments as you go:

```
$ orpa review
commit da05da11960b59249a286999612c1fcba90dbd19
Author: Joe Smith <joe@smith.net>
Date:   Fri Feb 12 19:09:27 2021 +0900

    Document the CLI options

 src/main.rs | 33 +++++++++++++++++++++++++++++----
 1 file changed, 29 insertions(+), 4 deletions(-)

> mark
da05da11960b59249a286999612c1fcba90dbd19: Reviewed
```

Both commands will accept a range, so you can use them with merge requests too:

```
$ orpa status 563e5fb..aadb1f9
563e5fb..aadb1f9: The following commits are awaiting review:

  9fbc3f8 Make the notes ref configurable
  aadb1f9 Use Lazy for CLI opts

Review them using "orpa review 563e5fb..aadb1f9"
```

## Advanced functionality

### Configuring `orpa fetch`

Orpa can load the open MRs from your MR tracker and display the unreviewed
commits in the same way.  Currently it only supports gitlab, but support
for other trackers could be added too.

Get an API token for your gitlab instance (read-only API access is enough),
and put a section like this in your local git repository's .git/config file:

```ini
[gitlab]
    url = "gitlab.com"
    privateToken = "1234567890abcdefgijk"
    projectId = "8765"
    username = "asayers"
```

### Listing merge requests

Let's grab the latest MRs from gitlab with `orpa fetch`:

```
$ orpa fetch
Fetching open MRs for project 1 from gitlab.example.com...
```

Now, `orpa status` is giving us some new information:

```
$ orpa status
Merge requests with unreviewed commits:

  !84    Add --notes-ref CLI argument (2 unreviewed)

Use "orpa mr" to see the full MR information
```

Let's take a look at it...

```
$ orpa mr 84
merge_request !84
Author: Joe Smith (@jsmith)
Date:   2019-12-10 08:42:20.768 UTC

    Add --notes-ref CLI argument

    v1 563e5fb..aadb1f9 (0/2 reviewed)
```

And there's the range we need to pass to `orpa review`!
