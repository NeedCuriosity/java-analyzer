import java.util.List;

class Main {
    void run(List<String> names) {
        String text = "text".substring(/*@begin*/1, /*@end*/2);
        names.add(/*@index*/0, /*@element*/"a");
        names.subList(/*@from*/0, /*@to*/1);
    }
}
//? hint index "index:"
//? hint element "element:"
//? hint from "fromIndex:"
//? hint to "toIndex:"
