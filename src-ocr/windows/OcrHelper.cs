using System;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading.Tasks;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using Windows.Storage;

namespace OcrHelper
{
    class Program
    {
        static async Task<int> Main(string[] args)
        {
            if (args.Length < 1)
            {
                Console.Error.WriteLine("Error: Missing image path argument.");
                return 1;
            }

            string imagePath = Path.GetFullPath(args[0]);
            if (!File.Exists(imagePath))
            {
                Console.Error.WriteLine($"Error: Image file does not exist at: {imagePath}");
                return 1;
            }

            try
            {
                StorageFile file = await StorageFile.GetFileFromPathAsync(imagePath);
                using (var stream = await file.OpenAsync(FileAccessMode.Read))
                {
                    BitmapDecoder decoder = await BitmapDecoder.CreateAsync(stream);
                    using (SoftwareBitmap softwareBitmap = await decoder.GetSoftwareBitmapAsync())
                    {
                        Language? swedish = OcrEngine.AvailableRecognizerLanguages
                            .FirstOrDefault(language => language.LanguageTag.StartsWith("sv", StringComparison.OrdinalIgnoreCase));
                        OcrEngine ocrEngine = swedish != null
                            ? OcrEngine.TryCreateFromLanguage(swedish)
                            : OcrEngine.TryCreateFromUserProfileLanguages();
                        if (ocrEngine == null)
                        {
                            Console.Error.WriteLine("Error: Could not create OcrEngine (no languages installed?).");
                            return 1;
                        }

                        OcrResult result = await ocrEngine.RecognizeAsync(softwareBitmap);
                        if (args.Length > 1 && args[1] == "--words-json")
                        {
                            var words = result.Lines.SelectMany(line => line.Words).Select(word => new
                            {
                                text = word.Text,
                                x = word.BoundingRect.X,
                                y = word.BoundingRect.Y,
                                width = word.BoundingRect.Width,
                                height = word.BoundingRect.Height
                            });
                            Console.WriteLine(JsonSerializer.Serialize(words));
                        }
                        else
                        {
                            Console.WriteLine(result.Text);
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"Error performing OCR: {ex.Message}");
                return 1;
            }

            return 0;
        }
    }
}
