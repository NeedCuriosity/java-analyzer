class Main {
    void move(int x, int y, String label) {}
    void move(int x) {}
    Main(String name, boolean visible) {}

    void run() {
        move(/*@first*/);
        move(1, /*@second*/);
        move(1, 2, /*@third*/);
        move(1, Math.max(/*@nested*/1, 2), "a");
        new Main("a", /*@ctor*/true);
        "text".substring(1, /*@lib*/2);
    }
}
//? sig second ~ "move(int x, int y, String label)" active 1
//? sig third ~ "move(int x, int y, String label)" active 2
//? sig nested ~ "max(int" active 0
//? sig ctor ~ "Main(String name, boolean visible)" active 1
//? sig lib ~ "substring(int" active 1
//? sig first ~ "move(" active 0
