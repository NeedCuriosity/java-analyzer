import java.util.List;

class Main {
    void run(List<String> names) {
        String.format(/*@format*/"%d", 1);
        "text".substring(1, /*@substring*/2);
        Math.max(1, /*@max*/2);
        names.add(/*@add*/0, "a");
    }
}
//? sig add ~ "add(int index, String element)" active 0
