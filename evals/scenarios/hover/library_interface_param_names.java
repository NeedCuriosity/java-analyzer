import java.util.List;
import java.util.stream.Collectors;

class Main {
    void run(List<String> names) throws InterruptedException {
        String text = String./*@format*/format("%d", 1);
        List<String> copy = names.stream()./*@collect*/collect(Collectors.toList());
        this./*@wait*/wait(10);
        int bigger = Math./*@max*/max(1, 2);
        String part = text./*@substring*/substring(1, 2);
    }
}
//? hover collect ~ "collector)"
