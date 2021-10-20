# INTERN

The Internal Network Topic-Exploring Researcher for Notes (INTERN) is a local search engine designed around my---[John](https://john.colagioia.net)'s---notes.

If you're asking yourself if the premise is at all based on [Monocle](https://github.com/thesephist/monocle), then yes, more or less.  The *extensive* press that it has gotten in the last few months spurred me to do something similar.  Here's an edited overview from [my developer journal blog post](https://john.colagioia.net/blog/2021/10/04/space.html) where I introduce the idea:

* * *

I have a lot of different kinds of notes, unfortunately, and they don't mix well.  My various archives include a variety that includes---but probably isn't limited to---the following.

 * These blog posts, written in Markdown.
 * Issues of [the blog's newsletter](https://entropy-arbitrage.mailchimpsites.com/), also written in Markdown.
 * Text files on various topics related to business and fiction ideas from as far back as twenty years ago, plus ideas branching from those ideas.  If I need a list of television stations derived from something that comes from Free Culture or public domain fiction *and* won't accidentally overlap with a real channel, that's in there, somewhere.  Oh, and while most of these collections are managed using [git](https://git-scm.com/), this collection is in a [Mercurial](https://www.mercurial-scm.org/) repository, because I liked it better back then and incorrectly thought it would become the industry standard.
 * Lecture notes from [my years of teaching]({% post_url 2020-01-19-teaching %}), mostly in Markdown, and also in a Mercurial repository.
 * The notes that I previously maintained with [Boost Note](https://boostnote.io/), but now maintain with my own [**Miniboost**](https://github.com/jcolag/Miniboost).  The notes are written in Markdown embedded in CSON, the [CoffeeScript](https://en.wikipedia.org/wiki/CoffeeScript) version of [JSON](https://en.wikipedia.org/wiki/JSON).
 * A "workbench" of different folders representing various non-code projects that I might be working on at any given time.  There are partial board games, works of fiction, role-playing games, translations, short stories, design documents for possible browser games, and so forth.  That's where I kept the original drafts for [**Seeking Refuge**]({% post_url 2019-12-14-seeking-refuge %}), for example.  Most of that is in Markdown, but some of it is not.
 * The usual "Documents" and "Downloads" folders, which could include just about any kind of file.  Unlike the other collections, these aren't managed under version control.
 * A couple of backups of twenty-year-old systems, which are snapshots of whatever I was working on and reading at the time, which I wanted to take another look at.  Again, these could include almost any format from the era.
 * A nightly journal to review what I've accomplished, what I'm eating, what I remember that I need to take care of soon, and anything else that might come to mind while I'm dumping out the rest of that information.
 * Code, some of which is downloaded for use, rather than something that I work on.  Most of this is text-based, but could be any programming language.
 * Current e-mail, in some variation of an [mbox](https://en.wikipedia.org/wiki/Mbox) format.

You get the idea.  When I remember that I've written something about whatever I'm about to work on, I need to figure out *where* I might have written about that and what program(s) I need to see the information as I originally intended it.  This isn't even the end of it.  What if the comment was in in the middle of a webpage that I read last year, or brought up in a chat on any number of services?

So, in the loose spirit of Linus Lee's [**Monocle**](https://github.com/thesephist/monocle), but completely unrelated---I haven't bothered to investigate its features---I'm going to start the Internal Network Topic-Exploring Researcher for Notes or [**INTERN**](https://github.com/jcolag/intern), a project to help me search this information in a way that's reasonable to me.  That will be my ongoing project, for the duration, probably written in [Rust](https://www.rust-lang.org/) until I discover why that will have been a terrible mistake and I abandon it for JavaScript or something else that I'll enjoy less but have more opportunities to use.  You'll notice that I use the word "me" a few times, there, and I mean that; this will almost certainly be based on my personal needs, with it only being a pleasant coincidence if it's ever useful to anybody else.  It's my **INTERN**.  You'll probably need to hire your own...

* * *

The upshot is that I would expect the folders and certain features to be configurable with an external configuration file, but I would **not** expect me to rush to add file types that I don't need---if I add file types at all---worry about platforms that I don't use, or otherwise allow this to grow too far outside my personal use case.

Eventually, though, I may consider adding content like visited webpages, since it's close to impossible to perform a keyword search, but limited to pages that I've seen at *some* point.

