import nook

app = nook.Plugin()
count = app.load("count", 0)


def draw():
    app.tile(nook.column([
        nook.text(count, size="title"),
        nook.row([
            nook.button("dec", icon="minus", tooltip="One less"),
            nook.button("inc", icon="plus", primary=True, tooltip="One more"),
        ], gap=2),
    ], gap=0, align="center", justify="center"))
    # Mirror the count into the collapsed pill once it is set.
    if count:
        app.badge_ring(min(abs(count) / 10, 1), str(count))
    else:
        app.clear_badge()


def change(by):
    global count
    count += by
    app.save("count", count)
    draw()


app.on("inc")(lambda e: change(1))
app.on("dec")(lambda e: change(-1))

draw()
app.run()
