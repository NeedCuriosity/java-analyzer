import java.util.List;

class Main {
    void run(List<String> names) {
        String.format(/*@format*/"%d", 1);
        "text".substring(1, /*@substring*/2);
        Math.max(1, /*@max*/2);
        names.add(/*@add*/0, "a");
    }
}
//? sig format ~ "format(String format, Object... args)" active 0
//? sig substring ~ "substring(int beginIndex, int endIndex)" active 1
//? sig max ~ "max(int a, int b)" active 1
